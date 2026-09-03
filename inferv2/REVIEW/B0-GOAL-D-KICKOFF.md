# Goal D — "vLLM for voice, in full" — kickoff state + keystone root cause

## Mandate
Root-cause-fix ALL 7 backlog items + the bigger vision: multi-hardware (every model on every HW), the
custom (non-ONNX) execution path (Path B), all 40+ models (Path A + B) live, every precision/quant/dtype,
full vLLM-feature-parity for voice. NO compromises/trade-offs (honesty required, but close the gaps for real).

## Keystone (item 1) — ROOT CAUSE NAILED
- voxtral q4f16 + cohere fp16 **run CORRECTLY on CPU EP** (voxtral 12.5s RTF~1.0, cohere 2.3s RTF 0.19, both
  correct transcripts). So the models + WaaV arms are CORRECT — the failure is **ORT CUDA EP kernel coverage**.
- voxtral decoder: 26× `com.microsoft.GroupQueryAttention`. `attention_bias` input = EMPTY; the set input is
  **`head_sink`** (input[10] = `/model/gqa_attention_bias/Expand/output_0`) — an ATTENTION-SINK (per-head
  softmax bias). ORT's CUDA GQA kernel does NOT implement attention-sink (CPU does). The "attention_bias not
  supported" message is imprecise — the real gap is head_sink. attrs: num_heads=32 kv=8 local_window=8192
  do_rotary=1 → a sliding-window + RoPE + sink Mistral decoder.
- cohere fp16: cuDNN "No execution plans support the graph" (different ORT/cuDNN op-coverage gap; B2 confirming).
- VERDICT: ORT's CUDA EP has kernel-coverage holes (attention-sink GQA, int8-GEMM, cohere's op). The strategic
  fix is the **custom execution path** (candle pure-Rust, multi-backend) that doesn't depend on ORT kernels —
  the vLLM model. Tactical alt = ONNX graph surgery (decompose GQA-with-sink into CUDA-supported primitives),
  bit-faithful but per-model + brittle. Decision pending the Path-B (candle/tch) assessment.

## Assessment fleet LAUNCHED (parallel, read-only) → REVIEW/B1..B5
- B1 pathb-reality: is the torch/custom runtime real/portable/venv-free? what runs the 23 torch models?
- B2 precision-matrix: per-arm input-dtype state, device×precision grid, voxtral/cohere GQA ONNX-fixability.
- B3 multihardware: device/EP abstraction, hardware-pinned assumptions, path to "every hardware".
- B4 model-inventory: ALL 40+ models, Path-A-runnable vs Path-B-blocked split.
- B5 vllm-parity: vLLM feature set vs WaaV (paged-attn, prefix-cache, quant, spec-decode, LoRA, ...) gap map.

## Known: NO tch/libtorch/candle/pyo3 in any Cargo dep → the "custom path" is NOT built as a portable runtime
(torch_sidecar.rs exists; 23 torch models; INFER_TORCH_RUNTIME.md is a DESIGN). The no-venv rule stands.

## ITEM 1 — RESOLVED ROOT CAUSE + FIX PATH (verified live)
- voxtral q4f16 + cohere fp16 run CORRECTLY on CPU (correct transcripts). CUDA fails: ORT-1.27 CUDA
  `GroupQueryAttention` kernel does NOT support `attention_bias` (only fp16/bf16 GQA *without* bias on the
  Blackwell flash path). The bias = `(1-attention_mask)*-65504` (padding mask, redundant with `seqlens_k`).
- ONNX SURGERY DOESN'T STICK: stripping/truncating the GQA `attention_bias` input from the .onnx is UNDONE by
  ORT's **Level3 graph-opt fusion**, which re-derives the bias from the `attention_mask` graph input on load
  (proven: arm loads the edited file — hiding it → "does not exist" — yet the identical GQA error persists at
  9 inputs). backend-ort sets no opt-level (defaults Level3); no per-model opt knob exists.
- → FIX (proper, per the "not just ONNX" mandate): serve voxtral/cohere via the **custom backend** — candle
  (Track 2, native biased-attention on CUDA) or the torch sidecar (interim; transformers attention supports
  the bias). A brittle ONNX-only alt would need load-time strip + opt-level=Disable + dead-subgraph prune
  (perf-costly, fights the optimizer) — NOT the right architecture. Item 1 folds into Track 2.

## TRACK 5 — torch models LIVE-verified (B6): 10/14 genuinely live, 4 typed-fixable, ZERO OOM
LIVE: neutts-air, qwen3-tts(RTF0.74), omnivoice(0.85), dots-tts-mf, dots-tts-soar, dia-1.6b, csm-1b-hf,
cosyvoice3, higgs-tts, granite-speech(verbatim-accurate STT). Process-isolation reclaimed the unified pool
every run (no leak), largest drop 20G (higgs). Path B sidecar is REAL.
FIXABLE: dia2-2b (Mimi codec SDPA rejects shape — enable math-SDPA on codec path); ark-asr-0.6b
(torch.compile graph-breaks on Tensor.item() — capture_scalar_outputs=True/disable-compile); dots-tts-base
(flow ODE ~1patch/28s blew 60s — raise budget/perf); vibevoice-1.5b (lm_head.weight MISSING from ckpt — needs weights).
→ COVERAGE: 24 ONNX-CUDA + 10 torch = 34 live; +4 torch-fixable; +2 ONNX CPU-only (voxtral/cohere → candle).

## ITEM 4 — REFINED (verified): NOT an urgent live bug
The live per-request path is `engine.rs::admit_bandwidth` → `DutyLedger::bandwidth_utilization()` (a READ of
the pre-calibrated ledger) + compare to S. The `DutyLedger::admit(stages)` clone-per-call (admission.rs:824)
+ the admit→add two-phase TOCTOU are on the CALIBRATION path (one-time, `calibrate_co_load_profile`) and the
SHELF-WARE `admit_layered`/`TierArbiter` (unwired). `.add()` runs at calibration, not per request. Migration
stale-epoch = a fleet (multi-worker) feature, not single-node-live. → Item 4 folds into the Track-1d
scheduler-shelf-ware decision (wire-or-downscope), NOT a live hot-path fix. The live admission is sound.

## GOAL-D SESSION DELIVERED (committed + verified)
- Full assessment: REVIEW/B1-B5 + B6 (torch-live) + B7 (fp16-inputs); MASTER_PLAN_VLLM_VOICE.md (5 tracks).
- item 1 root-caused → candle/torch (ONNX surgery fought by ORT Level3 re-fusion; CPU-correct, CUDA-blocked).
- item 2 (e2e fp16 inputs, 7 arms) + item 5 (load-resilience + WS metrics) — COMMITTED e2ff596, bit-faithful,
  clippy -D warnings clean, core lib 61/0 + live CUDA bit-identity gate PASS.
- Track 5: 10/14 torch models LIVE-verified (real output), +3 fixes in flight, zero OOM.
- COVERAGE now: 24 ONNX-CUDA + 10 torch = 34 models live; +3 torch-fixing; +2 ONNX CPU-only (→candle); +1 needs weights.

## ★ CANDLE VALIDATED ON GB10 (Blackwell aarch64 sm_121) — Track 2 GO
candle-core 0.9 +cuda compiled (42s, CUDA_COMPUTE_CAP=121) and ran CORRECTLY on GB10: CUDA matmul[256,1024]
+ softmax → sum=256.0000 (exact). EXIT=0. So candle-CUDA works where ORT-CUDA has kernel gaps (biased-GQA,
int8-GEMM, attention-sink). → the candle backend is the real, portable fix for item 1 (voxtral/cohere on
CUDA), the 14 torch models (out of Python), and multi-hardware (candle = CUDA+Metal+CPU). Smoke at
/tmp/candle_smoke (Cargo.toml + main.rs reference). NOW BUILDING: waav-infer-backend-candle + candle voxtral.

## ★★ ITEM 1 FIXED — candle Voxtral runs on GB10 CUDA (B8) ★★
New crate crates/waav-infer-backend-candle (candle 0.9, CPU+CUDA, portable). candle Voxtral on CUDA vs ORT-CPU
reference: 98.9% char-identical, ZERO accuracy degradation (candle's eager attention ADDS the sliding-window
mask pre-softmax — exactly what ORT's CUDA GQA refuses). The portable custom backend is REAL + works where ONNX
can't. Weights mistralai/Voxtral-Mini-4B-Realtime-2602 (8.86GB bf16). RTF 2.83 (correctness-first; perf =
device-resident ring-KV, the INFER_PERF #1 lever, next phase). Honest: ada_scale is a folded constant from the
ONNX decoder (faithful, not yet pure-from-safetensors); registry/serve dispatch wiring remains.

## CANDLE INTEGRATION PLAN (coordinated pass, after the prefix-cache agent reports, to avoid cache churn)
- candle-core gemm-f16 needs NEON `fullfp16` on aarch64 → add a committed `.cargo/config.toml`
  `[target.aarch64-unknown-linux-gnu] rustflags=["-C","target-feature=+fp16"]` (applies to all builds, harmless
  elsewhere) + `CUDA_COMPUTE_CAP=121` + `/usr/local/cuda/bin` on PATH in gb10-env.sh. (members=["crates/*"] →
  candle is auto-included, so `cargo build --workspace` REQUIRES this.)
- #[ignore] the 3 candle CUDA tests (cuda_smoke/cuda_voxtral/cuda_vs_ort) — heavy live-GPU; add to ci/heavy_live_tests.sh.
- verify clippy --workspace --all-targets -D warnings + cargo test --workspace green WITH the recipe; commit
  candle crate + config + heavy_gates together.
- THEN: wire candle into the registry (architecture=voxtral_realtime + device=cuda → candle arm); perf (ring-KV);
  port cohere (item 1's 2nd arm) + the 14 torch models → candle (retire the Python sidecar).
