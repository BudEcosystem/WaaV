# WaaV Infer — "vLLM for Voice" Master Plan (Goal D)
Mandate: root-cause-fix ALL gaps; multi-hardware (every model every HW); both paths (ONNX + custom);
all ~46 models live across every precision/quant/dtype; full vLLM-feature-parity for voice. No compromises.
Grounded in the 6-doc assessment: REVIEW/B1-pathb · B2-precision · B3-multihardware · B4-inventory · B5-vllm-parity.

## The governing insight (B5)
vLLM-for-voice gaps are **overwhelmingly SHELF-WARE, not missing ideas**: the parts (RadixPrefixCache,
PagedKvTable/RingKvCache, LoraRegistry, LayeredAdmission/Cohort/Tier, the resilience legs) are pub+tested but
have ZERO live callers. WaaV has the parts bin; it hasn't assembled the half that makes vLLM special. Plus the
"custom path" (Path B) is a **Python torch sidecar** (black-box, not portable), and only CUDA+CPU are proven.

## Current truth (baseline)
- ~46 models: **24 ONNX CUDA-live**, **2 ONNX CPU-only** (voxtral q4f16 + cohere fp16 — ORT CUDA attention-SINK
  kernel gap, both run perfect on CPU), **14 torch-sidecar (UNVERIFIED live)**, 3 blocked.
- vLLM parity: **4 HAVE / 7 PARTIAL / 6 MISSING**.
- Device abstraction first-class (10 EPs, load-dynamic) but GB10-hardcoded EpCaps, no live device enum.
- 5 CRITICALs already fixed this session (16209da). Item-5 admission metrics: wired (this commit).

## The 5 tracks

### TRACK 1 — ASSEMBLE THE vLLM PILLARS (wire the shelf-ware) — HIGHEST ROI, mostly wiring
- **1a Prefix-cache** RadixPrefixCache → live serve loop (the project's own 86% cloned-voice prefix-hit; #1 ROI).
- **1b Paged-KV** PagedKvTable/RingKvCache → live (long-form / many-turn escape; today contiguous per-slot ORT KV).
- **1c S-LoRA apply** — an arm actually applies adapter deltas (registry/lane routing already wired).
- **1d Scheduler fairness** — priority/preemption/fairness/graded-overload (live path is FCFS + bounded admit only).
- **1e Resilience legs** — crash-isolation/poison-firewall/dead-letter/recycle/sidecar-idle-scan → wired to a poller.
- → moves parity to ~8 HAVE / 4 PARTIAL / 4 MISSING. **Bit-faithful: accepted streams identical.**

### TRACK 2 — PORTABLE CUSTOM BACKEND (candle) — strategic centerpiece (the real "vLLM for voice")
- **2a** new crate `waav-infer-backend-candle` (pure-Rust; CUDA + Metal + CPU; no Python/venv) behind the
  existing `StaticGraph`/`SttModel`/`TtsModel`/`ArStepModel` seams (keep -core backend-agnostic, P-8).
- **2b** shared seams the design specs: AR LLM-decoder + ring/paged-KV + sampler; codec/vocoder decoders + CFM.
- **2c** port models: **voxtral + cohere FIRST** (fixes item 1 on CUDA via candle's flexible attention-sink),
  then the 14 torch models → candle, retiring the Python sidecar (no-venv compliance + folds them into WaaV's
  scheduler/ledger/prefix-router instead of HF model.generate() black box). Sidecar stays as bit-exact reference.
- → multi-hardware + item 1 + the torch models, all at once. BIG build (multi-week); start seam + voxtral POC now.

### TRACK 3 — BOUNDED BACKLOG FIXES
- item 1 voxtral/cohere CUDA: interim = add torch-runner OR ONNX graph-surgery (decompose sink-GQA); final = candle.
- item 2 e2e fp16: generalize `StaticGraph::input_types()`-driven INPUT-dtype casts to the 7 f32-pinned arms.
- item 4 scheduler hazards: DutyLedger single-critical-section admit+commit (no TOCTOU/clone-per-call); migration dest-epoch reject.
- item 5 metrics: admission DONE; remaining = batcher queue-depth + coalescer + WS-error counter.
- item 6 tests: handler-level WS-concurrency, chaos/fault-injection, fairness/starvation, oversized-input.
- item 7 batched TTFA: per-tick incremental decode in the mux loop (bit-identical concat).

### TRACK 4 — MULTI-HARDWARE
- live `DeviceCaps`/`devices()` enumeration → stop running on GB10 constants; model discrete-vs-unified memory.
- prove the wired EPs: x86-CPU/CUDA (near-free), then ROCm/OpenVINO/CoreML behind their ORT dylibs + a conformance gate.
- scope the GB10-only workarounds (kokoro CPU-pin, teardown SIGABRT mem::forget, 48GiB arena, TF32-off) to the substrates that need them.
- candle (Track 2) supplies Metal/Vulkan/Hexagon.

### TRACK 5 — VERIFY ALL 46 MODELS LIVE, EVERY PRECISION (iterative perf, zero accuracy loss)
- 14 torch models (sidecar): real forward + accuracy (vs provider) + perf + per-arm gates.
- every precision variant (fp32/fp16/bf16/int8/q4/q4f16) per model per HW; the QuantStamp gate honored.
- continuous perf improvement with bit-exact / WER-non-regression guard.

## Execution order (in-session, leverage-first; the rest is a scoped multi-session program)
1. Track 3 quick wins (item 5 finish, item 2 input-dtype, item 4, item 7) + Track 5 (verify torch models live) + item 1 interim.
2. Track 1 wiring (prefix-cache → paged-KV → LoRA-apply → scheduler-fairness → resilience legs).
3. Track 2 candle seam + voxtral POC (then amortize) + Track 4 device-enum.
4. Track 5 full precision×model×HW matrix + iterative perf.
Each step: bit-faithful, verified live, committed. Honest: the full candle migration + multi-HW proving spans
sessions — but every landed piece is real + tested; nothing is faked or shelf-ware.

## ITEM 3/4 — SHELFWARE DISPOSITION (wire-vs-downscope, the honest call)
WIRE (high-value vLLM pillars — the real work, in progress / next waves):
- prefix-cache (RadixPrefixCache, the 86% cloned-voice TTFA win) — Track-1 next wave.
- paged-KV (PagedKvTable/RingKvCache, long-form escape) — Track-1.
- S-LoRA apply (an arm applies adapter deltas) — Track-1.
- resilience legs (sidecar-idle-scan poller, poison-firewall, dead-letter, recycle) — wire to a live poller.
- scheduler fairness/priority/preemption — wire the live admission to the cohort/tier machinery.
DOWN-SCOPE AS ROADMAP (honest docs — NOT claimed live; wiring unused capability is not the priority):
- whole crates `router` / `provider` / `gateway-provider-api` — FLEET/gateway features (multi-worker
  prefix-affinity routing, the GW provider adapter). Not needed for single-node serving. Roadmap, documented.
- most of `features` (transport_egress/text_frontend/asr_post/feature_stage/streaming_encoder_cache) +
  `backend-api` policy API (StagePlacer/Relay/Shm/credit-pool/ngram-guard) — design-ahead; roadmap.
- `dag` except the CLI `run-dag` — the multi-stage DAG is CLI-only; the live path is direct STT/TTS. Roadmap.
- scheduler advanced admission (LayeredAdmission/Cohort/Tier/KvFirewall) — the live admission is the simpler
  bounded+VRAM+deadline gate (proven, sound). The advanced layer is roadmap (folds into the fairness wire above).
- DutyLedger clone/TOCTOU (item 4) — calibration/shelf-ware path, NOT live-hot (live = a read). Fix WHEN the
  advanced admission is wired; today the live admission is sound.
- S2S `CodecArDuplexModel` — a synthetic hash scaffold, NOT a real model. RELABELED honestly (corrects the
  #6 "native-S2S" claim, done in [[waav-infer-v2-production-ready]] memory + the verdict). Real S2S = a
  trained duplex model on the candle backend (Track 2 roadmap).
