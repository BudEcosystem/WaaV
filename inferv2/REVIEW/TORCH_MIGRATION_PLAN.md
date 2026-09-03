# WaaV Infer — Torch (tch-rs) Migration Plan (Goal D)
Move the 14 Python-sidecar models + the 2 candle item-1 stopgaps onto the in-process tch-rs runtime
(`waav-infer-backend-torch`), retiring the Python serving path. Leverage = 2 SHARED SEAMS (AR decoder, codec/CFM)
from INFER_TORCH_RUNTIME.md — port a seam once, every model on it is mostly config+weights after.

## The worklist → ~12 architecture families (not 16 ports)
| Family | models | seam | notes |
|---|---|---|---|
| voxtral_realtime (ASR, Mistral-dec) | voxtral | AR-decoder | **WAVE 1, in flight** — proves the runtime + AR seam |
| cohere (ASR, FastConformer enc + transf dec) | cohere | AR-decoder | candle stopgap → re-port; hybrid ORT-enc + tch-dec ok |
| ArkasrForConditionalGeneration (ASR LLM-dec) | ark-asr-0.6b | AR-decoder | LLM-decoder ASR |
| GraniteSpeech (ASR LLM-dec) | granite-speech-4.1-2b | AR-decoder | LLM-decoder ASR |
| csm (AR codec-TTS) | csm-1b-hf | AR-dec + codec | Sesame CSM |
| dia / dia2 (AR codec-TTS) | dia-1.6b, dia2-2b | AR-dec + codec | 2 ports (Mimi codec — the dia2 SDPA fix carries) |
| higgs_tts (AR codec-TTS) | higgs-tts | AR-dec + codec | |
| qwen3_tts (AR codec-TTS) | qwen3-tts-12hz-06b | AR-dec + codec | |
| dots_tts (AR codec-TTS) | dots-tts base/mf/soar | AR-dec + codec | **3 variants = 1 port** (same arch, diff weights) |
| neutts_air (AR codec-TTS) | neutts-air | AR-dec + codec | |
| cosyvoice3 (flow-matching TTS) | cosyvoice3 | CFM | proves the flow/CFM seam |
| vibevoice (diffusion TTS) | vibevoice-1.5b | CFM/diffusion | + the lm_head load-map fix |
| omnivoice (masked-diffusion LM) | omnivoice | CFM/diffusion | |

## Wave order (each wave gated on the prior seam landing + bit-faithful)
- **WAVE 1 — runtime + AR seam (IN FLIGHT, B16):** backend-torch crate (tch CUDA-link build.rs) + voxtral. The
  de-risk: does tch serve an AR model on GB10 CUDA, bit-faithful, RTF<1? Gates everything.
- **WAVE 2 — prove the 3 shared patterns (parallel, 3 agents):** one AR codec-TTS (dia2 — Mimi codec, the hardest
  codec, so it generalizes) + one ASR-LLM-decoder (ark OR cohere re-port) + one flow-TTS (cosyvoice3 = CFM seam).
  These 3 establish the AR-decoder, codec-decoder, and CFM seams as reusable WaaV components.
- **WAVE 3 — fan-out (parallel, batched):** the remaining families reuse the Wave-2 seams — csm/dia/higgs/qwen3/
  dots/neutts on the AR+codec seam; vibevoice/omnivoice on the CFM/diffusion seam; granite on the AR-decoder seam.
  Mostly per-arch decoder config + weights (like the ONNX registry's zero-code adds, but with the tch decode loop).
- **WAVE 4 — perf + accel:** Torch-TensorRT compile (via AccelMapper, NGC container for Blackwell) for the hot
  models; wire ring/paged-KV + the lockstep batcher onto the tch seam (the candle ring-KV patterns port to tch).

## Per-model LAW (every port)
- Bit-faithful: the tch transcript/audio MATCHES the current sidecar (and the reference engine) — byte/sample
  identical where the sidecar was, ≥ the banked accuracy bar otherwise. No fallback/approx/skip.
- RTF reported; target <1 (tch mature kernels + CUDA-graphs ≥ candle/sidecar).
- `#[ignore]` live-GPU gate vs the reference, registered in ci/heavy_live_tests.sh. clippy -D warnings clean.
- Reuse `waav_infer_components` (mel/codec/text) bit-for-bit; reuse the Wave-1/2 seams (no per-model kernel rewrite).

## Retirement
When a family is tch-live + bit-faithful, flip its `waav.json` backend torch→torch-inprocess (or drop the runtime
block so the engine routes to the tch arm). The Python `torch_runtime/` sidecar stays ONLY as bring-up/fallback
until all 13 families are tch-live, then it's removed (the no-venv goal achieved via the Torch ecosystem, not candle).
