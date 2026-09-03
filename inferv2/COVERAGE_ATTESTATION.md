# WaaV Infer v2.1 — Coverage Attestation

**Date:** 2026-06-17 · **Question answered:** *Does the architecture satisfy all 1,113 scenarios in full?*

## Method

An adversarial coverage audit: **10 skeptic auditors** (one per scenario family), each instructed to *find holes, not rubber-stamp*, read `INFER_ENGINE_V2.md` + `INFER_ENGINE_IMPL.md` + `INFER_FAILURE_CATALOG.md` in full and classified **every** scenario as:
- **SATISFIED** — a concrete mechanism handles it, cited to a §/R#/pillar **and** a named IMPL test-gate.
- **PARTIAL** — covered in principle but underspecified (a missing parameter, policy, gate, or interaction).
- **GAP** — no mechanism anywhere.

Per-scenario verdict tables: `WaaV/inferv2/scenarios/coverage/*.md` (one per family). Gap aggregation + 7-layer taxonomy: `WaaV/inferv2/INFER_COVERAGE_GAPS.md`.

## Pre-closure result (the honest baseline)

| Family | Total | SATISFIED | PARTIAL | GAP | SAT% |
|---|---:|---:|---:|---:|---:|
| 01 STT | 120 | 71 | 46 | 3 | 59% |
| 02 TTS | 130 | 106 | 19 | 5 | 82% |
| 03 S2S | 100 | 50 | 30 | 20 | 50% |
| 04 arch | 115 | 90 | 25 | 0 | 78% |
| 05 hardware | 121 | 32 | 79 | 10 | 26% |
| 06 batching | 114 | 90 | 19 | 5 | 79% |
| 07 scaling | 120 | 48 | 65 | 7 | 40% |
| 08 SLO | 105 | 56 | 38 | 11 | 53% |
| 09 failure | 118 | 86 | 21 | 11 | 73% |
| 10 features | 70 | 22 | 44 | 4 | 31% |
| **TOTAL** | **1,113** | **651** | **386** | **76** | **58.5%** |

**Verdict:** the **core thesis is satisfied and unchanged** (lockstep, two-batcher, hybrid-KV, numerics, hardening spine — the `MaskedCell` type-enforcement and post-graph NaN guard were cited as *exemplary*; the nested/third-class/cohort core returned **0 GAPs**). **Every shortfall was additive** — 34.7% PARTIAL ("named, not gated/specified") + 6.8% GAP ("no mechanism") — and **none required a core reframe.** The low-SAT families (hardware 26%, features 31%, scaling 40%) are exactly the layers the v2.0 doc *named but did not build*: the HAL was left as "v1.0 substrate," the DAG machinery and feature edges were assumed, and five control-plane subsystems were listed not specified.

## The 7-layer gap-closure (all folded into V2 §6 + IMPL §7)

| Layer | What was missing | Closed by |
|---|---|---|
| **1. Feature edges** | ingress-resample, text-frontend (SSML/locale-TN/code-switch), ASR-feature-postproc (alignment/confidence), transport-egress; non-core features (denoise/diarize/VAD/verify/KWS) as stage nodes | V2 §6.1, IMPL M2b |
| **2. DAG machinery** | dynamic `route_fn`/`wait_for_fn`/multi-terminal, FINAL-propagation, sentence-aggregator, DAG-wide reset, cloud stage, reliable DAG barge-in | V2 §6.2, IMPL M4.1b |
| **3. Duplex/multistream seam** | `ArStepModel` is single-stream → `DuplexStepModel`/`MultiStreamSlot`, EoT head, per-codebook depth, acoustic-delay→M2 | V2 §6.3, IMPL M2 |
| **4. Scheduler objective function** | principles, not a function → risk-EDF objective, binding-resource admission, **corrected nested NFE math**, bandwidth-duty measurement, router, tiers, feasibility-reject | V2 §6.4, IMPL M4.2/M4.3 |
| **5. Control plane / lifecycle / cross-cell** | 5 subsystems named-not-built; **cross-cell VRAM double-load** correctness gap; migration self-contradiction | V2 §6.5, IMPL M4.5 |
| **6. R6 reasoning cascade** | latency-filler/sentence-stream/two-tier/barge-in-cancels-LLM (was external-only) | V2 §6.6, IMPL M5 |
| **7. Guards / 3-tier KV / placement / precision** | StreamingEncoderCache (3rd KV tier), StagePlacer 11-test block, int8-not-on-ORT-CUDA, `EpKind::Hpu`, degeneracy/inner-NFE/in-flight-recycle guards, cycle-safe sniffer | V2 §6.7, IMPL M3/M4.x |
| **Hygiene** | sm120/sm121 reconcile; v1.0→v2.1 §-crosswalk; **restore 4 dropped v1.0 mechanisms** (drift-response, calibration-stamp, thermal, per-substrate accuracy/MOS) | V2 §6.0 |

## Post-closure state

After the V2 §6 + IMPL §7 patch: **every one of the 76 GAPs has a named mechanism, and every one of the 386 PARTIALs has a named test-gate or a concrete spec.** The completeness rule (IMPL §7.1) makes this a standing invariant: *every mechanism named in V2 maps to ≥1 IMPL gate; an un-gated mechanism is flagged `requires-gate-before-production` and may not ship.*

**Therefore:** the architecture now provides, for **all 1,113 scenarios**, `mechanism ≠ ∅ ∧ gate ≠ ∅`. The honest distinction that remains: this is **specification-complete coverage** (every scenario has a designed mechanism and a defined failing-test gate), not **implementation-verified coverage** (the gates are RED specs in the TDD plan, not yet GREEN code). The standing regression is a CI re-run of the 10 family auditors against the patched docs.

**Bottom line:** to your question — *not at v2.0* (58.5% satisfied, the rest additive holes), but **at v2.1, yes: every scenario is now addressed by a named mechanism with a named test-gate**, the core thesis held throughout, and the closure was additive specification, not a redesign.

## Convergence pass (the v2.1 closure, verified + deep-designed)

The v2.1 closure was specified at gate-*name* granularity and asserted "every scenario closed" *by construction*. To actually verify it (and remove the depth asymmetry vs the core seam), 6 per-layer agents each **(a) convergence-verified** their layer (adversarially: does the new mechanism compose with the lockstep core? did the patch introduce new gaps?) and **(b) deep-designed** it to real Rust types + RED test bodies (`WaaV/inferv2/design/*.md`, indexed at `WaaV/inferv2/design/00_INDEX.md`).

**Result — convergence confirmed:**
- **Composition with the lockstep core was PROVEN in every layer**, not asserted: the `DuplexStepModel` is the K=1 generalization (masked≠absent + one-graph-forever survive); the scheduler objective is computable (corrected max-NFE math validated by a worked 64-AR+8-CFM example — the old scalar-mean bug under-counted by 4 ms); the 3-tier KV is disjoint by writer/lifetime/owner.
- **Two confirmed fixes:** the migration self-contradiction (cadence=rejected vs fault=leased) and the cross-cell VRAM double-load race (box-scoped singleton).
- **The re-audit found exactly 2 new gaps** (`final_gate_tail_predicate_is_per_archetype`, `nested_stage_reset_slot_clears_inner_latent_before_outer`) — folded into IMPL §7.2 — and **1 real codebase debt** (the `EdgeResampler` lacks anti-alias on downsample today).
- All residuals are **cross-layer dependencies, orchestrator-owned-by-design, or wiring (method-exists)** — zero core-thesis contradictions.
- **~133 types + ~130 named gates (~108 full RED test bodies)** now span the v2.1 layers at the same depth as the core seam.

**Honest ceiling, restated:** this is now **convergence-verified, struct/test-body-deep specification** — the highest design fidelity short of running code. The gates are RED specs; turning them GREEN (M2→M5) is the engine-build execution phase, which this design enables but does not perform. The standing regression is the CI re-run of the auditors. Record: `WaaV/inferv2/INFER_CONVERGENCE.md`.
