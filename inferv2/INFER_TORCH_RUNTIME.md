# WaaV Infer — Path-B: a PyTorch Sidecar Runtime for non-ONNX voice models

**Status:** design (researched + grounded), 2026-06-14. Companion to `INFER_SPEC.md` / `INFER_REUSE.md`.
**Why:** ~20–30 model_list models ship or cleanly export to ONNX (served today by the Rust ORT/`StaticGraph`
backend). The remaining ~50 are *genuinely* non-ONNX — stateful autoregressive codec-token TTS, LLM-decoder
ASR, full-duplex S2S, framework-internal diffusion/codec — and will never load through `StaticGraph`. This
document designs the second runtime for them, grounded in how **vLLM-Omni** and **SGLang-Omni** (which serve
exactly these models — `higgs_audio_v3`, `fishaudio/s2-pro`, `qwen3-omni`, all on our list) actually work.

---

## 0. Decision (TL;DR)

1. **A PyTorch sidecar process** implementing WaaV's existing **`SttModel` / `TtsModel` behavioral contract**,
   spoken to over the existing native **WS v1 / UDS** protocol. **Not** PyO3 (would embed CPython + torch CUDA
   in the Rust process, violating P-5 crash-containment and the zero-C/C++ `-core` posture). **Not** a new
   `StaticGraph` impl (that seam is a *stateless single `run`*; these models are AR/KV/CFM loops that the spec
   deliberately keeps *in the engine*, not in the graph).
2. **Borrow the patterns, not the frameworks.** vLLM-Omni/SGLang-Omni are heavy (full vLLM/SGLang, paged-KV,
   RadixAttention, CUDA-graph capture, GPU-only) and not installed on this GB10 (aarch64+Blackwell, CUDA 13).
   We build a **minimal** torch runtime using their *model-definition interface* and *stage pipeline* ideas,
   and use their `models/*.py` as the **porting source** for each model.
3. **Kernel stack: torch eager + SDPA + `torch.compile(dynamic=False)` + bucketed manual CUDA graphs.**
   **No FlashInfer** (deferrable; vLLM-Omni's own Blackwell default *avoids* it — ~2× e2e regression on sm_120 —
   and AR voice never uses it; plus JIT/cubin-network risk). Triton only implicitly, via Inductor. This is
   exactly what vLLM-Omni's production voice code (`qwen3_code_predictor.py`: plain SDPA + fp32 RMSNorm/RoPE)
   does on this hardware class. Batch=1 voice is **launch/latency-bound, not FLOP-bound** → paged-KV/FlashInfer
   buy ~nothing.
4. **Two shared seams cover >80% of the non-ONNX models:** (1) a **Qwen/Llama AR LLM-decoder + KV cache**;
   (2) **codec/vocoder decoders** (Mimi, Higgs, DAC, AudioVAE/BigVGAN) + the **CFM solver already built for
   Supertonic**. Seed with **ARK-ASR-0.6B** (decoder seam) and **csm-1b** (Mimi codec); every later model
   amortizes onto those two.
5. **Verification is *stronger* here than on ONNX:** torch *is* the reference engine. The sidecar runs the real
   HF weights with ported model code; the reference is the same HF model via `transformers`/its repo, in the
   same process, on the local eval subset. No export drift → the goal's "verified against the inference engine
   the model uses" is the natural, default outcome.

---

## 1. Why a sidecar (grounded in INFER_SPEC)

- **P-5 — crash containment is a topology property, not a library property.** CUDA sticky errors and Python/
  C-extension `abort()` are *uncatchable in-process*. INFER_SPEC §4 already makes the **sidecar the default
  topology** for foreign runtimes. A torch process that SIGABRTs on a CUDA fault surfaces to the Rust engine as
  a **clean provider reconnect** (GW-3 breaker classification, NFR-R1) instead of taking down the gateway.
- **License posture (§16.4/§17.1).** torch + the models' Python deps (transformers, descript-audio-codec,
  hydra, …) stay **out of the Rust binary** as an arms-length separate program — the same line the spec already
  uses for `waav-g2p-espeak`. In-process embedding would forfeit the permissive-license + zero-C/C++ guarantee.
- **The Omni frameworks already are this shape.** vLLM-Omni = a python serving process; SGLang-Omni = the same.
  We're not inventing a topology; we're adding a *second provider* the engine already knows how to talk to.

## 2. The insertion seam (grounded in code)

`SttModel`/`TtsModel` (`waav-infer-core/src/model.rs:94-136`) are **pure behavioral contracts above the graph**:

```rust
trait SttModel:  transcribe(&mut self, pcm_16k: &[f32]) -> Result<String>;  set_language; supported_languages;
trait TtsModel:  synthesize(&mut self, text, voice, speed) -> Result<Vec<(ChunkMeta, Vec<i16>)>>;  voices; …
```

The `Engine` stores `Box<dyn SttModel>` / `Box<dyn TtsModel>` and **never touches a graph** (`engine.rs`). So a
torch-backed `TorchSidecarStt` / `TorchSidecarTts` that forwards calls over WS v1 and returns `String` / PCM
chunks **satisfies the same trait** and reuses everything above the seam *unchanged*: the registry, the
fixed-slot scheduler (FR-S2 §8.3), the device duty ledger (§8.3c), the protocol (`ChunkMeta`, `InferError`),
streaming, and the GW-1..GW-7 gateway. New wiring is small:

- A new `load_model` arm: when `waav.json` carries a `runtime: "torch"` block, build a sidecar-backed
  `SttModel`/`TtsModel` instead of composing `StaticGraph`s.
- A `TorchSidecarClient` (Rust) that owns the UDS connection + the breaker.

> The already-designed-but-unbuilt `LoadedModel` richer seam (INFER_SPEC §10.2: `StaticGraph | ArStep`) is
> *not* needed — the sidecar absorbs the AR loop on the Python side, so the Rust side stays at the clean
> `SttModel`/`TtsModel` level.

## 3. Division of labor — what stays Rust vs what moves to torch

| Layer | Home | Rationale |
|---|---|---|
| DSP frontends: `mel`, `nemo_mel`, `kaldi_fbank`, `stft` | **Rust** (above seam) | bit-exact-verified single source of truth; send PCM/features over IPC (≈100 KB/chunk) |
| Text frontend: `Segmenter`, `g2p`/misaki, `unicode_text`, `chunk_text` | **Rust** | pre-neural; canonical API surface |
| Notation/standardizer (`canonical_lang/precision/device`, `NotationMap`) | **Rust** | the public API contract; runs before the sidecar |
| Edge resample (`EdgeResampler`), protocol, scheduler, ledger, GW-* | **Rust** | topology-agnostic; reused verbatim |
| **The neural forward** (AR backbone, codec decode, sampling loop, diffusion steps) | **torch** | the un-ONNX-able part — and *only* this |
| Tokenizers/codecs the sidecar needs | **torch (Python ecosystem)** | HF `tokenizers`, the model's own codec lib — **duplicate, never FFI back to Rust** |

**Boundary contract:** STT = `PCM in → text out`; TTS = `text (or phonemes/features) in → PCM chunks out`.
This is exactly the `SttModel`/`TtsModel` boundary, so the IPC payload is trivial and DSP never duplicates.

## 4. The torch runtime internals

### 4a. Model-definition interface (mirror SGLang-Omni's 3-method shape)
A model is a Python class with **three methods + a string registry**, mirroring WaaV's config-arch dispatch:
```python
class FooTts(nn.Module):
    def __init__(self, config, dtype, prefix=""): ...      # build layers from a small shared layer lib
    def load_weights(self, hf_safetensors_iter): ...        # prefix-strip + per-param weight_loader (fused QKV/MoE)
    def generate(self, cond, sampling) -> AudioChunks: ...   # the AR/codec/CFM loop (streaming yield)
ENTRY = FooTts   # arch string (HF architectures[0]) -> class, same key WaaV already dispatches on
```
Weight loading: stream HF `safetensors` by `model.safetensors.index.json` weight_map, strip prefix, dispatch to
`weight_loader` for fused QKV/gate-up/MoE shards (SGLang's `weight_loader.py` pattern is directly portable).

### 4b. Seam 1 — AR LLM-decoder + KV cache
A small Qwen/Llama decoder with a **plain per-request torch KV cache** (no paged-KV — pointless at batch=1),
**SDPA** attention, **fp32 RMSNorm + RoPE** (numeric fidelity; vLLM-Omni makes the same choice), a top-k/top-p/
rep-penalty/RAS sampler in pure torch. Reused by: ARK-ASR, canary-qwen, higgs-stt, csm, higgs-tts, granite,
Mega-ASR. Multi-codebook TTS = a fused multi-embed/multi-head + delay-pattern mask on top of the same loop
(higgs/Fish pattern), with **persistent pre-allocated GPU buffers** so the inner codebook loop stays
CUDA-graph-safe.

### 4c. Seam 2 — codec/vocoder decoders + CFM
Plain `nn.Module` decoders with **chunked streaming** (`forward_chunk(codes, left_context, right_holdback)` +
overlap-add), loaded `strict=False` from a weight prefix:
- **Mimi** (Kyutai split-RVQ @12.5 Hz) → csm, MisoTTS, *all* S2S — highest reuse, build first.
- **Higgs** (8-codebook delay @25 fps) → higgs stt+tts pair.
- **DAC** (Descript) → ZONOS, Irodori, Semantic-DACVAE.
- **AudioVAE / BigVGAN** (continuous latent) → dots.tts, VoxCPM, Trendyol.
- **CFM solver** — *already built for Supertonic*; reused by CosyVoice3, Irodori, dots' per-step DiT head.

### 4d. Perf stack (GB10-validated pattern)
`torch eager` → wrap the decode step and the codec/vocoder loop in `torch.compile(dynamic=False)` → **bucketed
manual `torch.cuda.CUDAGraph` capture + replay** (pad to nearest captured bucket, slice output), warmed at load.
CUDA graphs on the **codec decode loop** are the single biggest first-token-latency lever (short fixed-shape
steps where launch overhead dominates). FlashInfer/hand-Triton: **deferred** — revisit only for a profiled
hotspot, or if real multi-request paged-KV batching is added later.

## 5. Constraints & their resolutions

| Constraint (from the seam audit) | Resolution |
|---|---|
| **Device duty ledger** (§8.3c) can't see the sidecar's VRAM → admission math wrong | sidecar **reports its VRAM footprint at handshake**; the Rust ledger subtracts it as a co-resident reserve (§4.2b makes double-booking explicit) |
| **Manifest is ONNX-file-shaped** (`{stem}_{precision}.onnx`) | add a `runtime:"torch"` block to `waav.json`: `{hf_repo, revision, dtype: bf16\|fp16\|int8-bnb\|awq\|gptq, codec_repo}`. FR-M1's artifact table is extensible |
| **Quantization** isn't ONNX-quant | torch path uses **bitsandbytes / AWQ / GPTQ** in-Python; per-quant verification = load the quantized HF weights and compare |
| **Teardown** (CUDA abort on exit, the `process::exit(0)` hazard) | *isolated by the sidecar* — its abort is a clean provider reconnect (GW-3), which is the whole point of P-5 |
| **Latency** of a second process | UDS + shared-memory PCM frames; the sidecar holds the model warm (compiled + CUDA-graph-captured) across requests |

## 6. Sidecar wire protocol
Reuse **WS v1 / UDS** frames; add message types `load(repo, runtime_cfg)`, `transcribe(pcm)->text`,
`synthesize(text, voice, speed)->pcm chunks (streaming)`, `health/vram`. To the breaker/scheduler the sidecar
is "just another provider" — reconnect/storm-control (W-D1/W-D2) already cover it.

## 7. Verification (preserves the "100% accuracy, reference-verified" bar)
For every torch model: run the **HF reference** (transformers or the model's repo) on the local eval subset →
compare to the sidecar's output. STT: token/WER match + WER vs ground-truth on a LibriSpeech subset. TTS: codec
tokens token-for-token (deterministic/greedy) and/or waveform correlation + a perceptual check. Because the
sidecar runs the *same weights + ported code*, this is bit/near-exact and **stronger than the ONNX path** (no
export drift). Keep the per-model eval scripts beside the existing `eval/*.py`.

## 8. Phased roadmap
- **P0 — skeleton.** Python WS-v1 sidecar server; Rust `TorchSidecarClient` + `TorchSidecar{Stt,Tts}`;
  `load_model` torch arm; ledger VRAM handshake. Prove end-to-end by re-serving an *already-done* model (e.g.
  whisper) through torch and matching the ONNX output — validates the seam with zero model risk.
- **P1 — Seam 1.** `ARK-ASR-0.6B` (cheapest LLM-decoder seed: Whisper-enc + MLP + Qwen2). Verify vs HF.
  Unlocks the ASR decoder family.
- **P2 — Seam 2.** `csm-1b` (Mimi RVQ decoder + depth-transformer). Verify vs HF. Unlocks the S2S/Moshi family.
- **P3 — amortize.** canary-qwen-2.5b, higgs-stt+tts (Higgs codec), Fun-CosyVoice3-0.5B + Irodori-600M (reuse
  CFM), granite-speech-2b, dots.tts (AudioVAE), hibiki-3b (reuse Mimi).
- **Throughout:** anything ONNX-exportable stays on ONNX (cheaper) — don't over-build torch.

## 9. Scope discipline — what NOT to build in torch
- **Prefer ONNX/export:** chatterbox, Qwen3-TTS, LFM2.5-Audio, VieNeu-TTS, MOSS-Nano, cohere-transcribe,
  nemotron-streaming, sortformer-diar, Voxtral-Realtime, DPDFNet, detail-co/clear, conv-tasnet, hush, LocalVQE,
  medasr. ONNX serves these more cheaply.
- **Skip:** >10B / MoE (SoulX-Transcriber 35B), NC-license (XTTS-v2, Voxtral-TTS), 7–9B duplex (PersonaPlex,
  Covo, VibeVoice-ASR) until the small seeds prove out, music/spatial (BS-RoFormer, Piano-Sep, helix,
  Ace-Step), niche VC/singing (RVC/Applio/Genshin, SoulX-Singer). For 7B Moshi/Helium specifically, the spec's
  **Candle/moshi-core reuse** (INFER_REUSE) is a better path than torch.

---

## 10. Production hardening — portability, performance, modularity, multi-model (course-correction)

**A model's pip package used as-is satisfies NONE of the production requirements** and is a *validation
harness only* (prove the math runs + generate a reference), never a deployment path. The four requirements and
how they're actually met:

| Requirement | Pip-package-as-is | The production answer |
|---|---|---|
| **Portability** (CUDA/ROCm/Hexagon/AMX/AVX/NEON) | CUDA-only, custom CUDA ops don't build elsewhere | Portability comes from the BACKEND: **ONNX+ORT EPs cover the whole list** (CUDA/ROCM/QNN-Hexagon/CPU-MLAS-AMX-AVX-NEON/CoreML/OpenVINO/TRT). Plain torch covers CUDA/ROCm/CPU (not DSP). Never a CUDA-locked package. |
| **Max performance / hw** | whatever the package has | Engine OWNS kernels per op per device: CUDA FlashInfer/Triton+graphs, ROCm hipBLAS/CK, CPU oneDNN/MLAS (AMX/AVX/NEON), NPU QNN. ORT does it via EPs; torch via `compile`+graphs+per-device SDPA. Impossible through a sealed package. |
| **Modular/reusable** | N packages/venvs, zero shared code = silos | Model = thin class composing SHARED layers (Linear/Attn/RMSNorm/RoPE/Embed) + shared decode loops + shared codec decoders + weights. vLLM `layers/`+`ModelRegistry`. Path-A already does this. |
| **One install, many models, no rebuild** | each model = its own install/venv | One install = engine + all kernels + all model classes; load = read-config→instantiate→load weights. Path-A already does this; the torch runtime must too (single env, NOT venv-per-model). |

**Deployment-form priority for any Path-B model:** (1) **export to ONNX (Path-A)** — full hw matrix +
single-install for free; (2) **transformers in ONE shared env** — a portable multi-model runtime (CUDA/ROCm/CPU,
one install loads many by config — where ARK-ASR + granite-speech already sit, ∴ closer to production than
chatterbox); (3) **ported model class in the shared torch runtime** (vLLM-style shared layers + weights) when
ONNX export is infeasible; (4) **per-model venv = LAST RESORT / validation only**, flagged as a portability+perf
compromise. The chatterbox pip-venv onboard is reclassified **"validated reference, not production-portable."**
Missing pieces to build for a real Path-B: **shared torch layers + shared codec decoders + a single-env torch
model registry** (no per-model venv). Re-tier the onboarding queue by PORTABILITY, not just "does it run."

## 10b. Distribution: load the PROVIDER's weights as-is (no re-publish) + multi-quant, the vLLM way

**ONNX-export means Bud RE-PUBLISHES weights → users depend on Bud's re-exports, not the provider's repo.**
That is NOT how vLLM works and is NOT the primary path. It corrects §10's "ONNX-export first" — re-export is a
last-resort/edge fallback only. The corrected distribution model:

1. **Primary: load the provider's ORIGINAL safetensors via a model-class registry, like vLLM.** Point WaaV at
   the provider's HF repo id → read config.json (architecture + quantization_config) → instantiate the
   registered model class → load the provider's original weights directly (map tensor names → shared layers).
   No re-publish, no per-user conversion. ARK-ASR + granite-speech ALREADY do this (`from_pretrained(provider/
   repo)`). Extend a model class per architecture.
2. **Multiple quantizations, two mechanisms (both "as-is"):** (a) load provider/community-published quantized
   checkpoints (AWQ/GPTQ/GGUF/fp8/bnb/int8/int4) — `transformers.from_pretrained` auto-detects via
   quantization_config; for ONNX the manifest selects the published int8/int4/q4 variant (e.g. nemotron-int4 is
   loaded AS PUBLISHED, not Bud-exported). (b) runtime quantization at load (fp8/bitsandbytes) — a
   `quantization=` knob like vLLM's `--quantization`.
3. **ONNX/ORT** is for loading provider/community-published ONNX as-is (onnx-community whisper/nemotron/…) +
   optional edge/NPU export — NOT a mandatory re-publish. Minimize Bud-exports (dasheng-style) to last resort.
4. **DON'T wrap the provider's inference code** (the MOSS trap: `inference_stream`→`torchaudio.save`→torchcodec
   dep-hell, non-portable). Load its WEIGHTS + run OUR forward on shared layers. Wrapping the package =
   anti-pattern; loading the weights = portable + as-is.

**Corrected onboarding hierarchy:** (1) model class loading provider's original safetensors [transformers where
it fits today — ARK/granite; ported class where it doesn't]; (2) load provider/community published ONNX OR
quantized checkpoints as-is; (3) Bud-export ONNX = last-resort/edge only. Usage target:
`waav serve <provider/repo> [--quantization awq|gptq|fp8]` — just loads, like vLLM.

## 10c. Never depend on a vendor-locked runtime (NeMo/NIM) — portability + paywall

NeMo is **NVIDIA-hardware-locked** (CUDA/cuDNN/Apex/TransformerEngine/Megatron + NVIDIA CUDA kernels; NO
ROCm/Hexagon/CoreML/OpenVINO, no portable CPU for most models). NVIDIA **NIM** are commercial microservices under
**NVIDIA AI Enterprise licensing**, distributed via **NGC** (account/entitlement); some NeMo checkpoints/deps are
gated (e.g. `nv_one_logger` is NOT on public PyPI — NVIDIA private index). So a vendor framework fails BOTH the
portability bar AND open/free distribution. **∴ WaaV must NOT use NeMo (or any vendor-locked runtime) as a
backend.** For vendor-origin models: take the OPEN WEIGHTS and run on a PORTABLE backend — published open ONNX →
ORT EPs (CUDA/ROCm/QNN/CoreML/OpenVINO/CPU), or a ported torch class. WaaV ALREADY does this right: parakeet +
canary-180m + canary-1b are served via istupakov's **open ONNX** (no NeMo, no NVIDIA license, runs anywhere ORT
runs). canary-qwen failed correctly because no open portable artifact exists yet → use/produce ONNX or port,
never install the framework. Generalizes to any vendor-locked runtime (CoreML-only/QNN-only/etc.). The engine
owns ONE portable runtime + loads open weights/exports.

## 11. Bottom line

WaaV Infer becomes a **two-runtime engine behind one model contract**: Path-A (Rust/ORT
`StaticGraph`) for ONNX-shaped models, Path-B (torch sidecar) for stateful AR/codec/duplex models — both
plugged in at the `SttModel`/`TtsModel` seam, both reusing the same DSP, scheduler, protocol, ledger, and
gateway. The torch runtime is deliberately minimal (eager + compile + CUDA graphs, two shared seams), ports
model defs from vLLM-Omni/SGLang-Omni, and verifies against the HF reference in-process. ARK-ASR + csm-1b are
the two seed models that build the decoder and codec seams everything else amortizes onto.
