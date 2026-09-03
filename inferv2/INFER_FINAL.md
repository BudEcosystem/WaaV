# WaaV Infer — Finalized Architecture & Spec (Master Index)

**Status:** FINAL (v2.1, perf-integrated) · **Date:** 2026-06-18 · **Device of record:** GB10 (Grace Neoverse-V2 + Blackwell sm_121). This is the authoritative entry point. It states the engine in one page, maps the canonical document set, and records the verification status. Everything here is **specification-complete, convergence-verified, and empirically perf-measured** — the highest design fidelity short of running code.

---

## 1. The engine in one page

**WaaV Infer is the "vLLM for voice": a realtime STT/TTS/S2S inference engine (<10B models) that runs the same model contract from a B200 datacenter to a GB10 edge box to a Hexagon phone — exact-by-default, KISS, config-tiered.**

- **Batching (the core).** Frame-synchronous **lockstep** for the AR spine (fixed slots, exec-mask over a rectangular batch, per-slot ring KV, wall-clock paced — *the* throughput lever, **55×@64 measured**) + **step-bucket** for diffusion/flow heads (CFG-folded, length-bucketed) + a **nested third class** (AR-outer + variable-NFE generative-inner) for DiTAR/FlashTTS. Cohort by `(model, stride)`; variable-stride for dynamic-frame-rate codecs.
- **KV (two-tier + encoder tier).** A shared paged **radix prefix-cache** (ref-audio/system prompt, ~86% hittable, bit-identical reuse, ~7× TTFA) + a per-slot **ring suffix** (bounded, coalesced, native GQA layout) + a **streaming-encoder delta cache** (STT). Not paged-for-the-suffix.
- **Scheduler (a computable function).** *maximize Σ viable-sessions s.t. ΣU≤bound ∧ Σbandwidth≤ceiling, ordered by risk-of-violation, shed by criticality+age.* Non-preemptible whole-stream admission; graded degradation (shed-LO → brownout → EDF+slack-drop → reject last); the playback buffer protects cadence.
- **Pipeline.** A heterogeneous **stage-DAG** (feature-edges in → core → feature-edges/transport-egress out) with decoupled per-stage batching, zero-copy heterogeneous placement on unified memory, dynamic fan-in/FINAL-propagation, and a control-plane/orchestrator split.
- **Performance (exact-only).** Perf = **batching + memory-bandwidth physics + the right exact kernel** — **zero custom kernels required.** Pin SDPA→cuDNN/flash (40–135×), IoBinding KV on the `StaticGraph` seam (13%→2×, the #1 engine change), native GQA (5.5–6.9×), batch-tiered CUDA-graph (edge only). No quant/spec-decode/approx by default. The fp32-fusion-survives + AR-compounding-identity gate guards every change.
- **Production spine (day-one).** Clockwork serialize-GPU, cell/shard fault isolation, box-scoped VRAM accountant, cooperative-cancel-every-frame + frame-progress watchdog + GPU-health sidecar, NaN→reject-frame, media-on-UDP/QUIC, coordinated-omission-honest observability, lifecycle FSM + control-plane.
- **Portability.** Path-A (Rust + ONNX Runtime, every EP) + Path-B (torch sidecar, CUDA/ROCm/CPU). The 16-arm config-arch registry ("model = data") is untouched by all of the above.

**The thesis, validated:** voice fixes the unknown (per-request token rate) that vLLM's continuous batching exists to manage → lockstep is the right spine, and it structurally dodges vLLM's four worst scars *and* its custom-kernel requirement.

---

## 2. Canonical document map (read in this order)

| Doc | What it is | Status |
|---|---|---|
| **`INFER_FINAL.md`** (this) | the finalized master index + one-page architecture | FINAL |
| **`INFER_ENGINE_V2.md`** | **the architecture** (§0 thesis, §1 the 5 reframe corrections, §2-§5 the layers, §6 the audit gap-closure, §7 the performance architecture) | FINAL — READ FIRST after this |
| **`INFER_ENGINE_IMPL.md`** | **the implementation plan** (M2→M5 milestones, §7 v2.1 gap-closure gates, §8 perf gates + the model-implementation contract + per-model fixes) | FINAL |
| **`INFER_GUIDELINES.md`** | **code patterns + engineering rules** every model/backend PR follows (the 2 invariants, the stepped-seam contract, masked≠absent, the exact-perf rules, backend selection, the forbidden list) | FINAL |
| **`INFER_PERF.md`** + `INFER_PERF_BENCH.md` | the accuracy-preserving perf strategy + the raw GB10 measurements (8 levers, 3 benchmark batches; scripts `/tmp/perf_bench/bench_perf_{1,2,3}.py`) | FINAL |
| **`design/00_INDEX.md`** + `design/L1-L7.md` | the deep low-level designs (~133 Rust types, ~130 RED test bodies) for the v2.1 layers | FINAL |
| **`INFER_FAILURE_CATALOG.md`** | 238 exactly-cited production scars (Moshi/SGLang/vLLM/vLLM-Omni + real-world spine) — the evidence base | reference |
| **`scenarios/00_INDEX.md`** + 10 family files + `coverage/` | 1,113 real-world scenarios (the coverage oracle) + the per-family audit verdicts | reference |
| **`COVERAGE_ATTESTATION.md`** / `INFER_CONVERGENCE.md` / `INFER_COVERAGE_GAPS.md` | the audit (651/386/76 → all closed), the convergence verification, the gap source | reference |
| `INFER_ENGINE.md` | v1.0 — the empirical substrate (§1 the 7 GB10 benchmarks, §2 HAL) the above build on; its serving-layer thesis is superseded by V2 | substrate |
| `INFER_SPEC.md` / `INFER_TORCH_RUNTIME.md` / `INFER_REUSE.md` | the foundational v1.0 spec, the Path-B sidecar design, the build-vs-borrow catalog | substrate |

---

## 3. The build sequence (M2→M5, each a shippable milestone)

The current 6-crate WaaV Infer has correct backend seams (`StaticGraph`, EP HAL, `SttModel`/`TtsModel`, registry, torch sidecar) but no serving discipline — that's the build. Each milestone is RED-first (every failure-case + audit-gap + perf-lever is a named test gate).

- **M2 — stepped seam + lockstep + the engine perf core.** `ArStepModel` seam; fixed-slot masked lockstep (masked≠absent gates first); per-slot ring KV (Kyutai wraparound vectors); acoustic-delay ring; **IoBinding `run_bound` on `StaticGraph`** (the #1 perf change); SDPA-backend-pin; native-GQA layout; batch-tiered CUDA-graph; Path-B HF StaticCache+compile per runner. *Accept:* ≥16 concurrent codec-AR streams at RTF<1; single-stream edge unchanged; the perf levers measured exact.
- **M2b — feature edges.** Ingress-normalizer / text-frontend / asr-postproc / transport-egress / FeatureStage taxonomy.
- **M3 — streaming + numerics + hybrid 3-tier KV + precision.** Delta-streaming + explicit FINAL; NaN→reject; the radix prefix-cache; the fp32-fusion-safe compile discipline + the accuracy gate.
- **M4 — stage-DAG + scheduler-function + control-plane + production spine.** Dynamic DAG routing; the computable scheduler objective + duty/bandwidth ledger; control-plane + lifecycle FSM + box-scoped VRAM accountant; StagePlacer + zero-copy; the watchdogs + cell isolation.
- **M5 — variable-stride + MTP + full-duplex + transport.** DuplexStepModel + EoT + the Kyutai-systems checklist; R6 reasoning cascade; fault-migration; media-on-UDP/QUIC.

---

## 4. Verification & empirical status

- **Coverage:** all **1,113 scenarios** audited → 651 SATISFIED / 386 PARTIAL / 76 GAP at v2.0; **all closed at v2.1** (every scenario has `mechanism ≠ ∅ ∧ gate ≠ ∅`). `COVERAGE_ATTESTATION.md`.
- **Convergence:** the v2.1 closure was re-audited + deep-designed per-layer — **composition with the lockstep core proven** (not asserted), 2 new gaps found+folded, residuals all cross-layer/wiring. `INFER_CONVERGENCE.md`.
- **Performance:** **8 exact levers empirically measured on the GB10** (`INFER_PERF_BENCH.md`); the kernel-physics verdict (zero custom kernels) proven in source.
- **Evidence:** 238 cited production scars; 10 deep source/literature studies; 6 perf studies.
- **Honest ceiling:** this is **specification-complete + convergence-verified + perf-measured** — the gates are RED specs; turning them GREEN (executing M2) is the engine-build phase this package enables but does not perform. The standing regression is a CI re-run of the scenario auditors against the docs.

**Bottom line:** the architecture is final and self-consistent — lockstep-as-fast-path inside a variable-stride, three-tier-KV, deadline-graded, cell-isolated, datagram-transported, coordinated-omission-honest engine, made fast by batching + bandwidth physics + exact kernel selection (zero custom kernels), correct by the masked≠absent + accuracy-gate disciplines, and complete against 1,113 scenarios. The next action is **build M2.**
