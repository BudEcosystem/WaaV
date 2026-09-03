# WaaV Infer v2.1 — Deep low-level designs (convergence pass)

**Date:** 2026-06-18 · The v2.1 gap-closure (V2 §6 + IMPL §7) was at gate-NAME granularity. These 6 docs take each layer down to **real Rust types + algorithms + RED test bodies**, and each agent *convergence-verified* its layer (proved composition with the lockstep core; hunted new gaps). Totals: **~133 types/traits, ~130 named gates (~108 full test bodies)**, ~4,200 lines.

| Doc | Layer | Closed / Residual | Key proof / finding |
|---|---|---|---|
| `L1_feature_edges.md` | Feature edges (ingress/text-frontend/asr-postproc/transport-egress + FeatureStage) | 28 / 9 (0 defects, 0 new gaps) | 3 patch-hazards checked+avoided; found real `EdgeResampler` no-anti-alias TODO; `WordTiming`/`keyterms` already exist |
| `L2_dag_machinery.md` | route_fn/wait_for_fn/FINAL/aggregator/DagSlotReset/CloudStage | 9 / 2 cross-layer | **Found 2 NEW gates** (Res-3 per-archetype FINAL tail predicate; Res-4 inner-before-outer reset) |
| `L3_L6_duplex_reasoning.md` | DuplexStepModel/EoT/acoustic-delay + R6 reasoning cascade | 13 / 6 cross-layer | **Proved** K=1 generalization preserves masked≠absent + one-graph; ComputeLease barge-in no-race |
| `L4_scheduler_function.md` | computable objective + ledger + router + tiers | 12 / 5 (wiring) | **Proved** objective computable; corrected max-NFE math via worked 64-AR+8-CFM example (scalar-mean bug under-counted 4ms) |
| `L5_control_plane.md` | engine↔orchestrator contract + lifecycle FSM + cross-cell ledger + migration | 31 / 6 (orchestrator-owned) | **Fixed** cross-cell VRAM double-load race (shown) + migration self-contradiction (leased, free-after-ACK) |
| `L7_guards_kv_placement.md` | 3rd KV tier + StagePlacer + precision + EpKind::Hpu | 13+3 / 0 | 3-tier KV coherent (disjoint writer/lifetime/owner); `StaticGraph::input_types()` already exists; Gaudi fixed by formula |

## Convergence verdict

- **Composition with the lockstep core was PROVEN in every layer**, not asserted (the duplex K=1-generalization, the cross-cell VRAM race, the computable objective with the worked example, the 3-tier KV disjointness).
- The re-audit found exactly **2 new gaps** (L2 Res-3/Res-4, both additive gates now in IMPL §7.2) and **1 real codebase debt** (the resampler anti-alias TODO).
- **Two confirmed fixes:** the migration self-contradiction and the cross-cell VRAM double-load correctness bug.
- **All residuals are cross-layer dependencies, orchestrator-owned-by-design, or wiring tasks (method-exists)** — zero core-thesis contradictions.

## What these are (and are not)

These are **specification-deep designs**: object-safe Rust traits/structs/enums in the existing crate idiom (typed errors not panics, `NopLoader`/`tmp_with_config` test-doubles), with representative RED test bodies mapped 1:1 to the `INFER_ENGINE_IMPL.md §7` gates. Most are **GPU-free unit tests**. They are the bridge from the architecture (`INFER_ENGINE_V2.md`) to GREEN code: an implementer reads the layer doc, drops the types into the named crate, makes the RED tests pass. They are **not yet compiled/GREEN** — that is the M2→M5 execution phase.
