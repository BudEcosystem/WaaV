# WaaV Infer v2 — Index (read-me-while-coding)

This folder is the **complete, finalized WaaV Infer v2 design package**: the architecture, spec, implementation plan, code-pattern guidelines, deep low-level designs, the empirical perf strategy, and the full evidence/coverage base. It is **specification-complete, convergence-verified, and perf-measured** — the gates are RED specs; building them GREEN (executing M2) is the next step.

**If you read one file: `INFER_FINAL.md`** (the one-page architecture + the canonical map). **If you're about to write code: `INFER_GUIDELINES.md`** (the rules every PR follows) + the relevant `design/Lx_*.md`.

---

## When to read what (by what you're doing)

| You are… | Read, in order |
|---|---|
| **Orienting / onboarding to the engine** | `INFER_FINAL.md` → `INFER_ENGINE_V2.md` (§0 thesis, §1 the 5 corrections) → `INFER_ENGINE.md` §1 (the 7 measured benchmarks) |
| **About to write ANY model or backend code** | `INFER_GUIDELINES.md` (the 2 invariants + the model contract + masked≠absent + the exact-perf rules + the forbidden list) — non-negotiable |
| **Planning the build / picking the next milestone** | `INFER_ENGINE_IMPL.md` (M2→M5 + the gate matrix; §8 = perf gates + the model-impl contract) |
| **Implementing a specific subsystem** | the matching `design/Lx_*.md` (real Rust types + RED test bodies) — see the design map below |
| **Adding a new AR/codec-LM/duplex model** | `INFER_GUIDELINES.md` §1 (the stepped-seam contract R-1…R-8) → `INFER_ENGINE_IMPL.md` §8.1/§8.3 → `design/L3_L6_duplex_reasoning.md` |
| **Optimizing performance (without hurting accuracy)** | `INFER_PERF.md` (the strategy + the per-hardware matrix) → `INFER_PERF_BENCH.md` (the measured numbers) → `INFER_GUIDELINES.md` §3 (the rules) |
| **Debugging a production/correctness issue** | `INFER_FAILURE_CATALOG.md` (238 cited scars — find your failure mode) → the relevant `design/Lx` |
| **Checking a scenario is handled / writing an integration test** | `scenarios/00_INDEX.md` → the family file → `scenarios/coverage/<family>_coverage.md` (the mechanism + gate that handles it) |
| **Verifying nothing was hand-waved** | `COVERAGE_ATTESTATION.md` + `INFER_CONVERGENCE.md` (the audit + convergence record) |

---

## The files

### Start here
| File | Contents | Purpose · when to read |
|---|---|---|
| **`INFER_FINAL.md`** | the engine in one page; the canonical doc map; the M2→M5 build sequence; verification status | the authoritative entry point — **read first, return to it for orientation** |

### Architecture (the *what* and *why*)
| File | Contents | Purpose · when to read |
|---|---|---|
| **`INFER_ENGINE_V2.md`** | THE architecture: §0 the 7-claim thesis, §1 the 5 evidence-backed reframe corrections (hybrid-KV, variable-stride+3rd-class, deadline-graded, KV-length firewall+spatial-P/D, heterogeneous-residency+MTP), §2-§5 the layers, §6 the audit gap-closure (the 7 layers specified), §7 the **performance architecture** | the design of record — **read before changing any architectural decision** |
| `INFER_ENGINE.md` | v1.0 substrate: §1 the 7 measured GB10 benchmarks (the empirical keystone), §2 the HAL. Its serving-layer thesis is *superseded* by V2; §1/§2 stand | read §1/§2 for the measured foundations the architecture rests on |

### Implementation (the *how* — test-first)
| File | Contents | Purpose · when to read |
|---|---|---|
| **`INFER_ENGINE_IMPL.md`** | the extreme-TDD plan: M2→M5 milestones, the failure-case→test-gate matrix, §7 the v2.1 gap-closure gates, **§8 the perf gates + the model-implementation contract (R-1…R-8) + per-onboarded-model fixes** | **read when picking what to build next** — every gate is RED-first |
| **`INFER_GUIDELINES.md`** | the standing rules every PR follows: the 2 invariants, the stepped-seam contract (with Rust shape), the masked≠absent law, the exact-perf rules, the per-hardware backend-selection table, streaming/lifecycle/ops rules, the **forbidden list** | **read before writing code; keep open while coding** |

### Deep low-level designs (real Rust types + RED test bodies — read the one you're building)
| File | Subsystem | When to read |
|---|---|---|
| `design/00_INDEX.md` | the design map + convergence summary | before opening any Lx |
| `design/L1_feature_edges.md` | ingress-resample, text-frontend (SSML/TN/code-switch), ASR-feature-postproc, transport-egress, the `FeatureStage` taxonomy + `StageState::reset`, BiasContext | building the pipeline bookends / non-core features |
| `design/L2_dag_machinery.md` | `route_fn`/`wait_for_fn`/multi-terminal, FINAL-propagation, `SentenceAggregator`, `DagSlotReset`, `CloudStage` | building the stage-DAG / feature composition |
| `design/L3_L6_duplex_reasoning.md` | `DuplexStepModel`/`MultiStreamSlot`, EoT head, `AcousticDelayRing`, the R6 reasoning cascade (latency-filler, LlmStreamNode) | building full-duplex S2S / cascade / the AR seam |
| `design/L4_scheduler_function.md` | the computable scheduler objective, `DutyLedger`, `RiskSlack`, the corrected max-NFE admission math, `Router`, SLA tiers | building the scheduler / admission |
| `design/L5_control_plane.md` | the engine↔orchestrator contract, `LifecycleFsm`, the box-scoped `VramAccountant`, leased `Migration` | building lifecycle / autoscaling / multi-cell |
| `design/L7_guards_kv_placement.md` | `StreamingEncoderCache` (3rd KV tier), `StagePlacer`+zero-copy, precision resolver, `EpKind::Hpu`, the shared guards | building the HAL placement / KV tiers / precision |

### Performance (accuracy-preserving; measured)
| File | Contents | Purpose · when to read |
|---|---|---|
| **`INFER_PERF.md`** | the exact-perf strategy: the framing answer (zero custom kernels), the measured-lever table, the ranked catalog, the per-(hardware×path) matrix, per-model opportunities, the accuracy gate, milestone mapping | **read before any perf work** |
| `INFER_PERF_BENCH.md` | the raw GB10 micro-benchmark data (8 levers, 3 batches; scripts at `/tmp/perf_bench/bench_perf_{1,2,3}.py`) | the empirical evidence behind every perf claim |

### Evidence & coverage (reference)
| File | Contents | Purpose · when to read |
|---|---|---|
| `INFER_FAILURE_CATALOG.md` | 238 exactly-cited production scars (Moshi/SGLang/vLLM/vLLM-Omni + the real-world day-one spine) | **the bug-prevention bible — consult by failure mode** |
| `scenarios/` | 1,113 real-world scenarios in 10 MECE families (`00_INDEX.md` + `01_stt`…`10_features`) + `coverage/` (the per-family audit verdicts: each scenario → mechanism → gate) | the coverage oracle — check your component handles its scenarios |
| `COVERAGE_ATTESTATION.md` | the audit result (651/386/76 → all closed) + the convergence-pass record | proof every scenario has a mechanism + a gate |
| `INFER_CONVERGENCE.md` | the per-layer convergence verification (composition proven, 2 new gates found, residuals = cross-layer/wiring) | proof the v2.1 gap-closure holds |
| `INFER_COVERAGE_GAPS.md` | the 7-layer gap taxonomy (the source the closure was built from) | trace why a layer exists |

### Substrate (v1.0 foundational — the lineage V2 builds on)
| File | Contents | Purpose · when to read |
|---|---|---|
| `INFER_SPEC.md` | the v1.0 governing spec (§8 scheduler, §8.3c duty ledger, §9 memory/lifecycle, §10.2 backend HAL) | when V2/IMPL says "extends INFER_SPEC §x" |
| `INFER_TORCH_RUNTIME.md` | the Path-B torch-sidecar design (the §4 model-def interface, §5 the VRAM/duty handshake) | when working on the Path-B sidecar |
| `INFER_REUSE.md` | the build-vs-borrow catalog (the OSS components to vendor vs write) | when deciding to reuse vs build a component |

---

## Directory layout
```
inferv2/
├── 00_INDEX.md                 ← this file
├── INFER_FINAL.md              ← start here (master index + 1-page architecture)
├── INFER_ENGINE_V2.md          ← the architecture (incl §7 performance)
├── INFER_ENGINE_IMPL.md        ← the TDD plan (M2→M5 + §8 perf+contract)
├── INFER_GUIDELINES.md         ← code patterns + rules (read while coding)
├── INFER_PERF.md               ← exact-perf strategy
├── INFER_PERF_BENCH.md         ← measured GB10 data
├── INFER_FAILURE_CATALOG.md    ← 238 production scars
├── COVERAGE_ATTESTATION.md     ← the coverage audit (all closed)
├── INFER_CONVERGENCE.md        ← convergence verification
├── INFER_COVERAGE_GAPS.md      ← the gap taxonomy
├── INFER_ENGINE.md             ← v1.0 substrate (measured benchmarks §1, HAL §2)
├── INFER_SPEC.md               ← v1.0 governing spec (substrate)
├── INFER_TORCH_RUNTIME.md      ← Path-B sidecar design (substrate)
├── INFER_REUSE.md              ← build-vs-borrow catalog (substrate)
├── design/                     ← deep low-level designs (00_INDEX + L1,L2,L3_L6,L4,L5,L7)
└── scenarios/                  ← 1,113 scenarios (00_INDEX + 10 families) + coverage/ (10 audits)
```

**Build order:** read `INFER_FINAL.md` → `INFER_GUIDELINES.md` → start `INFER_ENGINE_IMPL.md` **M2** (the stepped seam + lockstep + the IoBinding `run_bound` engine change), writing the masked≠absent gates first, with the matching `design/Lx` open.
