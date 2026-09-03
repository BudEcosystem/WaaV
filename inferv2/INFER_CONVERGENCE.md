# WaaV Infer v2.1 — Convergence + deep-design tracking (6 layers)
Each layer: convergence-verify (closed/residual + adversarial composition proof) + deep design (types/tests).

## L3+L6 Duplex + Reasoning — CLOSED 13 / residual 6 · 24 types, 15 tests (14 gates) · design/L3_L6_duplex_reasoning.md
- Composition PROVEN: DuplexStepModel = single→multi generalization (K=1 special case), K+D fold inside one [B,…] forward → masked≠absent + one-graph-forever survive; per-codebook StepOutput threads via acoustic-delay ring; barge-in ComputeLease returned under admission lock (no race).
- Residuals all cross-layer DEPENDENCIES not L3 holes → assign: variable-NFE T_step (L4), reasoning placement + prefix-router (L4/L5), cloud-stage barge-in ACK (L2), TurnHead trained weights (onboarding), inner-monologue paged-escape (L7), 3-way-collision golden-vectors (impl task).

## L2 DAG machinery — CLOSED 9 / residual 2 cross-layer + 3 adversarial findings · 22 types, 13 tests · design/L2_dag_machinery.md
- 2 NEW gaps the v2.1 patch introduced (real convergence value — NOT by-construction):
  - Res-3 [NEW GATE] `final_gate_tail_predicate_is_per_archetype` — FINAL tail_drained predicate differs vocoder(crossfade) vs AR(marker-heap); generic predicate truncates final chunk.
  - Res-4 [NEW GATE] `nested_stage_reset_slot_clears_inner_latent_before_outer` — DagSlotReset must clear inner-solver latent inner-BEFORE-outer atomically (G8 stale-batch landmine). Corroborates L3 + arch-audit.
- Res-5 route_fn static-topology blocks N-lang per-span routing = CORRECT conservative behavior → resolved by L5 LoRA-per-language (constraint boundary, not defect).
- 2 cross-layer residuals: per-span langID (needs L1 langID + OnLanguageChange boundary), time-aligned join (needs L3 active-set hook).
## ⇒ ADD AT CONSOLIDATION: 2 new gates to IMPL §7 completeness table.

## L4 Scheduler objective function — CLOSED 12 / residual 5 (all low/med, wiring not design) · 18 types, 16 tests · design/L4_scheduler_function.md
- BIGGEST audit concern ("principles not a function") RESOLVED. Adversarial ALL pass:
  - Objective COMPUTABLE — every term = calibrated scalar | live counter | config const.
  - Corrected max-over-NFE PROVEN by worked example: 64-AR+8-CFM(NFE{2,4})+codec+STT → T_step=0.025s≤0.08s, rejects-at-bandwidth-saturation (scalar-mean bug under-counted 4ms).
  - risk-slack + criticality-shed + age = total order, no double-decision (layered: tier→feasibility→admit→order→shed, each at a different point).
- Residual #1 (genuine): DRAM_ACTIVE live scrape→bytes_touched is M4.4 wiring (method exists per J23, admission-safe fallback). #2-5: route_herd body / shared predictor / strict-vs-work-conserving tier / masked-slot budget-branch — all KISS boundaries.

## L1 Feature edges — CLOSED 28 / residual 9 (all cross-layer, 0 L1 defects, 0 NEW gaps) · ~30 types, 34 tests · design/L1_feature_edges.md
- 3 patch-introduced hazards CHECKED+AVOIDED w/ gates: (Ha) TextFrontend doesn't perturb prefix fingerprint (text→ring suffix, key over (conditioning,bias) → 86% share survives); (Hb) TransportEgress off-clock DSP, bandwidth-only charge, no AR-tick serialize; (Hc) FeatureStage::reset ∘ DagSlotReset via one StageState trait.
- Real codebase debt found: EdgeResampler has NO anti-alias on downsample (TODO) → gates fail for a real reason (new AntiAliasResampler). WordTiming + SessionConfig.keyterms ALREADY EXIST (use, not new). New crate waav-infer-features (GPU-free).

## L5 Control plane — CLOSED 31 / residual 6 (all orchestrator-owned/cross-layer by design) · 24 types, 33 gates (17 bodies) · design/L5_control_plane.md
- Engine↔orchestrator line CLEAN (engine="can I now on this box?" yes/no/why; orchestrator=fleet loop) → KISS, no replica-map/autoscale in engine.
- CROSS-CELL VRAM double-load race FIXED (shown: 2 cells each see 60GiB, both malloc 40→OOM; box-scoped singleton reserve() µs critical section, lock released before malloc → serving tick never stalls).
- Degraded↔Ready hysteresis consumes L4's single band (no own threshold → no contradiction; trip>recover+dwell, blip re-arms).
- Leased fault-migration: free STRICTLY after dest-ACK, source single-writer → no zombie/double-admit on lease-expiry. Cadence-vs-fault contradiction RESOLVED.

## 5 of 6 in. AGGREGATE: composition PROVEN every layer (not asserted); residuals = cross-layer deps + orchestrator-owned + wiring; only 2 NEW gates introduced (L2 Res-3/Res-4). Migration contradiction FIXED, cross-cell VRAM bug FIXED. Awaiting L7.

## L7 Guards/KV/placement — CLOSED 13 / 3 layer-boundary / 0 residual · ~15 types, 19 gates · design/L7_guards_kv_placement.md
- 3-tier KV COHERENT: StreamingEncoderCache ∘ radix-prefix ∘ ring-suffix disjoint by (writer,lifetime,owner); only coupling = channel_id recycle fan-to-all-3.
- StaticGraph::input_types() ALREADY EXISTS (backend-api:177); empty_kv_dtype() maps name→declared-dtype, f32-feature inputs excepted BY NAME (q4f16 threads).
- roofline_class a-priori (L7) vs measured bandwidth_duty (L4) — coarse/fine, no conflict.
- Gaudi mis-model FIXED by formula: knee=f(flops_bw_ratio) → wide knee, no special-case; EpCaps{batch_profile:Wide} + EpKind::Hpu.

## ============ ALL 6 LAYERS CONVERGED ============
## Closed scenario-classes ~109 (→ the ~462 prev-PARTIAL/GAP individual scenarios); composition PROVEN every layer (NOT asserted).
## Totals: ~133 types/traits, ~130 named RED gates (~108 full test bodies) across 6 design/ docs.
## NEW gaps the v2.1 patch introduced: 2 (L2 Res-3 final_gate_tail_predicate_is_per_archetype; L2 Res-4 nested_stage_reset_slot_clears_inner_latent_before_outer).
## FIXED: migration self-contradiction (L5), cross-cell VRAM double-load race (L5), scheduler "principles→computable function" (L4, 4ms-bug worked example).
## Residuals: ALL cross-layer-dependency | orchestrator-owned-by-design | wiring (method-exists) — ZERO core-thesis contradictions. + 1 real codebase debt (L1 EdgeResampler anti-alias TODO).
