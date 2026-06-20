# WaaV Infer — Real-World Scenario Catalog (Master Index)

**Status:** complete · **Date:** 2026-06-17 · **1,113 scenarios** across 10 MECE families · authored by a 10-agent fleet, each grounded in `INFER_ENGINE_V2.md` (the reframed architecture), `INFER_ENGINE.md` (the empirical substrate), and `/tmp/waav_failure_catalog.md` (238 exactly-cited production scars).

> **Purpose.** This catalog is the **coverage oracle** for the architecture: the optimal long-standing WaaV Infer design must address every scenario here, the most optimal way, while staying KISS. Every entry is a *real situation the engine actually faces* — the relevance bar was enforced hard (a bad scenario derails the architecture, so no padding, no speculation, no duplicates within a family). The catalog goes **simple → intermediate → compound → extreme**, ending each family in multi-axis compounded failure cases.

## Counts

| File | Family | Scenarios | Prefix |
|---|---|---:|---|
| `01_stt.md` | Speech-to-Text pipeline | 120 | STT- |
| `02_tts.md` | Text-to-Speech pipeline | 130 | TTS- |
| `03_s2s.md` | Speech-to-Speech / full-duplex / cascade | 100 | S2S- |
| `04_arch.md` | Multi-architecture / nested-paradigm | 115 | ARCH- |
| `05_hardware.md` | Multi-hardware / hardware-architecture | 121 | HW- |
| `06_batching.md` | Batching / scheduling / worker / stage-DAG | 114 | BAT- |
| `07_scaling.md` | Scaling / multi-tenancy / deploy / lifecycle | 120 | SCALE- |
| `08_slo.md` | Request-prioritization / SLO / QoE | 105 | SLO- |
| `09_failure.md` | Failure / recovery / production-hardening | 118 | FAIL- |
| `10_features.md` | Feature-composition / multi-feature DAG | 70 | FEAT- |
| **TOTAL** | | **1,113** | |

## Difficulty spread (the simple→extreme mandate)

| Level | Count | Share |
|---|---:|---:|
| Simple | 183 | 16% |
| Intermediate | 309 | 28% |
| Compound | 350 | 31% |
| Extreme | 272 | 24% |

Compound + Extreme = **622 (56%)** — the catalog is deliberately weighted toward the compounded/multi-axis failure cases that stress the architecture, with each family climaxing in an "everything-at-once" capstone (e.g. BAT-105 the 64-AR + 8-CFM + codec + STT mixed-clock worker with a mid-tick slot recycle; HW-81 the AR-on-GPU + codec-on-CPU-AMX + STT-on-NPU one-LPDDR-bus box; S2S-100 full-duplex multilingual translation + barge-in + reasoning-stall + slot-churn + long-context on a shared GB10; SCALE-120 the everything-at-once ops day; FEAT-64 the far-field multilingual meeting-assistant DAG).

## Axis coverage (the dimensions the user mandated, by tag frequency)

753 distinct axis-tags appear; the load-bearing categories:

| Axis category | Hits | Axis category | Hits |
|---|---:|---|---:|
| `hw:` (CPU/GPU/NPU/HPU/GB10/…) | 311 | `seqlen:` (short/long/stream) | 120 |
| `arch:` (AR/flow/diffusion/masked/nested/MTP) | 301 | `lang:` (multilingual/codeswitch) | 120 |
| `feat:` (translate/enhance/diarize/clone/…) | 269 | `scale:` (edge/DC/autoscale) | 103 |
| `batch:` (lockstep/step-bucket/micro/cohort) | 212 | `dag:` | 81 |
| `slo:` (TTFA/throughput/e2e/session) | 211 | `priority:` (realtime/batch) | 78 |
| `mem:` (unified/HBM/GDDR/paged/ring) | 182 | `worker:` (multi) | 64 |
| `fail:` (OOM/crash/NaN/jitter/leak/contaminate) | 172 | `simd:` (HMX/AMX/SIMT) | 59 |

Plus a long tail of specific tags (precision, lifecycle, capacity, fault, qoe, co-residency, frame-rate, transport, numerics, voice-clone, …) — every dimension the prompt named (multi-architecture, multi-modality, multi-hardware, multi-hardware-architecture/SIMD-SIMT/memory-hierarchy, multi-pipeline, multi-feature, multi-worker, heterogeneous, scaling, sequence-length/language/encoder/IO, and request-prioritization for UX/throughput/e2e/TTFA/per-session) is exercised, and most are compounded across families.

## Schema (every scenario)

```
### <PREFIX-n> — <title>
- Level: Simple | Intermediate | Compound | Extreme
- Pipeline: <STT|TTS|S2S|batch|enhance|translate|diarize|…>
- Axes: <comma-tagged dimensions exercised>
- Scenario: <the concrete real situation>
- System must: <the optimal KISS handling>
- If mishandled: <the failure / architectural implication>
```

## How the families partition (MECE)

Families are **mutually exclusive by primary concern** and collectively exhaustive over the engine's surface. A scenario that touches several axes lives in the family of its *primary* concern and tags the others (so cross-family overlap is intentional cross-referencing, not duplication): a "voice-clone TTS under multi-tenant overload on GB10" lives in TTS (primary), tagging `feat:clone`, `scale:DC`, `hw:GB10`, `slo:`. The `## Coverage` section at the end of each file records what it deliberately deferred to a sibling family.

## How to use this catalog

1. **Architecture validation:** every load-bearing decision in `INFER_ENGINE_V2.md` must address its relevant scenarios. The reframed-architecture corrections (R1 hybrid-KV, R2 variable-stride + third class, R3 deadline-graded degradation, R4 KV-length firewall + intra-node P/D, R5 heterogeneous residency / MTP / long-form escape) each trace to dozens of extreme scenarios here.
2. **TDD gating:** the extreme scenarios are integration-test targets for `INFER_ENGINE_IMPL.md`; the failure-mode `If mishandled` lines map to the named test gates in that plan's matrix.
3. **Regression net:** as the engine is built, each scenario becomes a row in a conformance checklist (must handle / handles / how).

## Companion documents

- `INFER_ENGINE_V2.md` — the reframed v2.0 architecture (read first).
- `INFER_ENGINE.md` — the empirical substrate (§1 benchmarks, §2 HAL) + the original v1.0 thesis it supersedes.
- `INFER_ENGINE_IMPL.md` — the extreme-TDD implementation plan (failure-case → test-gate matrix).
- `/tmp/waav_failure_catalog.md` — the 238-entry exactly-cited production-scar corpus all of the above draw from.
