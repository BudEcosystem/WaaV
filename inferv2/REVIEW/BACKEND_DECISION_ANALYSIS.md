# WaaV Infer — Execution-Backend Decision Analysis (Goal D)
Decision-grade analysis for "vLLM for voice, every hardware, max perf, no compromises". Research + LIVE GB10
evidence (this session). User steer: keep ONNX + Torch; **drop candle as a standalone**; evaluate tch-rs + Torch-TensorRT.

## The reframe that governs the choice
The vLLM-for-voice optimizations — continuous batching, ring/paged-KV, prefix reuse, priority/fairness
scheduling, attention kernels, quant — must be BUILT BY US on ANY substrate (no backend gives them free).
So the decision = which substrate makes building+running them cheapest, fastest, widest-HW, least-risky.

## Substrate comparison
| Axis | ONNX RT | PyTorch (Python) | tch-rs / libtorch (Torch C++) | candle (Rust) |
|---|---|---|---|---|
| Hardware | CUDA/ROCm/TensorRT/OpenVINO/CoreML/**QNN-AOT**/DirectML — broadest no-Python + edge | CUDA/ROCm/**TPU**/MPS/XPU + ExecuTorch — broadest overall | libtorch's: CUDA/ROCm/CPU/MPS | CPU/CUDA/Metal/WASM — **no ROCm/TPU** |
| Attn/kernel flexibility | LOW (graph ops; voxtral GQA-bias wall) | HIGHEST (FlexAttention, Triton) | High (ATen ops; no FlexAttention/compile) | High (own kernels; fewer; no CUDA-graph in 0.9) |
| Perf ceiling | fixed-graph good; generative poor (host-KV) | HIGHEST (compile + CUDA-graph + TRT) | High (cuBLAS/cuDNN + CUDA-graph via libtorch) | Medium (own kernels; no CUDA-graph) |
| Deployment | no-Python C++; edge AOT | Python (unless ExecuTorch/AOTInductor) | **no-Python** (2GB C++ dep + crash surface) | **best — static Rust binary** |
| Our build-effort | fight the graph | lowest (FlexAttention/compile) | medium (loop on ATen) | highest (reimplement every kernel) |
| Maturity | very mature, kernel-gappy | most mature | mature kernels; thin bindings | youngest (we hit bugs) |

## tch-rs verdict — MATURE ENOUGH + PROVEN ON GB10 (this session)
- Active (v0.24 Mar-2026), 5.4k★, dual-licensed; **Burn builds on it** (production). Full API: Tensor, nn::Module,
  CUDA, TorchScript JIT, **safetensors**, autograd.
- **GB10 live test:** built + ran on aarch64. CPU ✅. **CUDA ✅** (`cuda available: true, device_count:1, Cuda(0)
  matmul OK`) once `libtorch_cuda`/`libc10_cuda` are linked (LD_PRELOAD now; a `-ltorch_cuda` build flag for prod).
  The initial CPU-only result was the known `LIBTORCH_USE_PYTORCH` aarch64 auto-link quirk → one-time fix, NOT a blocker.
- Caveats: version-coupled (tch 0.24↔libtorch 2.11; box has 2.12 — bypassed; match for prod); thin bindings (write
  the decode loop on ATen — the candle ring-KV/zero-copy patterns port ~1:1, on PyTorch's mature kernels + CUDA-graphs;
  NO torch.compile/FlexAttention, those are Python-authoring); ~2GB libtorch dep + in-process C++ crash surface
  (mitigate with the existing out-of-band sidecar watchdog/reaper).
- → VIABLE as the in-process, no-Python "Torch (CPP)" runtime.

## Torch-TensorRT — the NVIDIA-CUDA PERF LAYER (not a runtime by itself)
- Production v2.12.1 (Jun-2026), up to 5× vs eager. **No-Python deploy: AOT → TorchScript `.ts`, run via
  libtorch/tch-rs without Python.** Quant FP16/INT8/FP8 + **NVFP4 (Blackwell)** — far better than ONNX's gappy CUDA
  quant. aarch64 ✅. AR/LLM decode ✅ via fixed-size-KV (= our ring-KV).
- NVIDIA-CUDA-ONLY (a perf accelerator, not portability). NOT installed on GB10; on Blackwell sm_121 pip is finicky →
  NGC container / source build (bleeding-edge). General path = static graphs; dynamic decode via the fixed-KV trick.
- Sibling **TensorRT-LLM** = full vLLM-equivalent for NVIDIA (paged-KV + in-flight batching + C++ runtime) — heaviest,
  LLM-shaped; only if max NVIDIA LLM perf is wanted.

## RECOMMENDED STACK (candle dropped)
- **ONNX RT** — broadest no-Python HW + edge AOT (Qualcomm QNN, NPUs). Fixed-graph models. KEEP.
- **Torch (Path B):**
  - **tch-rs / libtorch** — in-process, no-Python, Rust runtime (CPU/CUDA/ROCm). Decode loops on ATen. PROVEN on GB10.
  - **Torch-TensorRT** — AOT-compile hot models → TRT engine (.ts, no-Python via libtorch). FP8/NVFP4, 5×. CUDA perf layer.
  - **PyTorch (Python)** — authoring + FlexAttention/compile + the existing sidecar (bring-up/fallback; serves 12 models).
- **candle — RETIRED.** voxtral/cohere candle arms = item-1 stopgap → re-port to tch-rs (ATen ops ~1:1; perf patterns transfer).

## MIGRATION PLAN (on confirmation)
1. `waav-infer-backend-torch` crate (tch-rs + the CUDA-link fix + out-of-band crash supervision).
2. re-port voxtral + cohere candle→tch (prove the runtime; reuse ring-KV/zero-copy-gemm).
3. move the 14 sidecar models in-process → tch.
4. layer Torch-TensorRT AOT-compile for the hot models (FP8/NVFP4 on Blackwell via NGC).
5. wire the (now-landing) optimizations — batching, ring/paged-KV, priority/fairness, resilience — onto the tch seam
   (they're BACKEND-AGNOSTIC; built in -runtime/-scheduler/-server regardless of the substrate).
Open sub-decisions: tch-rs in-process vs supervised-sidecar for crash-containment; whether to adopt TensorRT-LLM for NVIDIA LLM max-perf.

Sources: github.com/LaurentMazare/tch-rs · github.com/pytorch/TensorRT · github.com/NVIDIA/TensorRT-LLM
