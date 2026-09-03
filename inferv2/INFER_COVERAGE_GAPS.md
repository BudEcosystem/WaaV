# WaaV Infer v2 — Coverage-audit gap aggregation (patch source)
Aggregating GAP/PARTIAL findings from the 10 adversarial coverage auditors. Patch INFER_ENGINE_V2.md + INFER_ENGINE_IMPL.md once all 10 are in, deduped.

## Running tally (satisfied / partial / gap)
- 02_tts: 106 / 19 / 5  (of 130)

## Cross-cutting gaps (appear across families — patch ONCE)

### CG-A [from TTS, likely also features/STT] — Missing first-class **TextFrontend stage** (pre-phoneme).
The architecture models codec-tokens-onward in detail but text-IN hand-wavily. Add a typed `TextFrontend` stage node to V2 §4 with per-model manifest capability flags: SSML (prosody/`<break>`/phoneme), locale number/date/currency normalization (ITN/TN), code-switching (Unicode-script segmentation → per-run G2P → joined), punctuation. Degrade rule: strip-to-plain, NEVER speak tag literals. IMPL gates: `ssml_tags_map_or_passthrough_never_spoken`, `locale_normalization_per_synthesis_language`, `code_switch_script_segmentation`. Closes TTS-15/30/40/113 (GAP) + 14/28/91 (PARTIAL).

### CG-B [from TTS, likely also S2S/features/scaling] — Missing first-class **TransportEgress stage** (post-vocoder).
V2 pillar-7 names "media on UDP/RTP" but the actual STAGE (anti-alias resample → G.711/Opus encode → jitter-buffer repacketize to fixed 20ms RTP) lives only in MEMORY. Add `TransportEgress` stage (post-codec, OFF the AR clock, CPU/NPU-placeable). IMPL gates: `downsample_anti_alias_always_on`, `repacketize_to_fixed_20ms_rtp`, `opus_inband_fec_on_loss`. Closes TTS-11/12/33/34/92.

## Per-family specific gaps

### TTS (02)
- GAP TTS-95: output-degeneracy/repetition-loop guard — rolling n-gram over emitted codec tokens → terminate+FINAL+metric (complements repetition_penalty=reference). → V2 pillar-5 guard + IMPL `frame_level_repetition_loop_broken`. [also caps TTS-106 runaway]
- PARTIAL TTS-106: per-slot `max_inner_steps_per_tick` cohort budget → reject-frame the offending slot (R3 sheds at admission, not per-slot-inner-step; a runaway inner-NFE solve paces all). → V2 R2/R3 + IMPL gate.
- PARTIAL TTS-114: NPU static-shape AOT contract (pad/segment variable codec output to compiled fixed chunk before NPU call; VTCM spill) — only in MEMORY. → V2 §2.3 `static_shape_required` substrate flag.
- PARTIAL TTS-121/125: multistream delay-table engine semantics (`(offset+delay)%context`, delay-sign = task-mode STT/TTS/S2S/translate) live in §9/MEMORY not main V2/IMPL text. → promote to V2 §4 multistream node schema + IMPL M5 gates.

- 08_slo: 56 / 38 / 11  (of 105)  [HARSH — scheduler was principles not a function]

## More cross-cutting gaps (SLO audit)

### CG-C [CRITICAL] — Scheduler OBJECTIVE FUNCTION must be SPECIFIED, not principles.
The six competing objectives are arbitrated by rules scattered across R3/§2/§4 with the only concrete key = "deadline-EDF." Make it COMPUTABLE: **"maximize Σ viable-sessions subject to ΣU≤bound ∧ Σbandwidth≤ceiling, ordered by RISK-slack (VoxServe risk-of-violation, NOT deadline-EDF alone), shed by criticality+age."** V2 cited VoxServe risk-ordering as evidence but never adopted it as the key (SLO-24). Add to V2 §6 + IMPL gates: `scheduler_orders_by_risk_not_deadline_alone`, `admit_iff_every_substrate_duty_le_S`, `admit_iff_shared_bandwidth_duty_le_ceiling`, `binary_viability_yields_slack_when_safe`, `drift_detector_fires_on_sustained_bottleneck_p99_breach`, `shed_selects_newest_least_progressed_realtime`. Closes SLO-3/5/23/24/33/34/39/40/62/63/99.

### CG-D [HIGH] — Prefix-affinity ROUTER is a HOOK, not a component.
The diagram names "prefix-affinity route" but there's no fleet ref-KV residency map, no routing function, no affinity-vs-duty arbiter. Add a `ROUTER` box + a small `waav-infer-router` concern: fleet ref-KV residency map → route a returning voice to the worker holding its prefix KV (the R1 86% hit), yielding to duty when the holder is saturated. IMPL: `routes_returning_voice_to_ref_kv_holder`, `affinity_yields_to_duty_when_holder_saturated`. Closes SLO-30/57/93.

### CG-E [HIGH] — FIX the migration self-contradiction in V2.
V2 line 38 disclaims migration ("cadence protected ... not migration") yet SLO-68/94 + scaling-spill REQUIRE it. Split: **cadence-migration = rejected** (the playback buffer protects steady cadence); **fault/spill-migration = REQUIRED** (append-only KV, playback-buffer-masked, admission-throttled rebalance — Llumnix-style ~20-30ms = >1 frame so it MUST be buffer-masked). Reword R3/§6 + IMPL gate `fault_migration_appends_kv_buffer_masked`.

### CG-F [HIGH] — Reasoning-LLM realtime cascade folded in as R6 (it's WaaV's existing REALTIME_REASONING work, currently external-only).
V2 treats a slow/reasoning LLM only as a "shed-LO" label. The cascade needs: latency-filler masking (fire filler when LLM TTFT exceeds budget), sentence-by-sentence LLM→TTS streaming aggregation, two-tier fast+reasoning, background-tier scheduling, barge-in-cancels-LLM (reclaim leftover). Add R6 "slow-tier masking" to V2 + M5 gates `filler_fires_when_ttft_exceeds_budget`, `barge_in_cancels_llm_reclaims_leftover`, `sentence_aggregation_streams_first_clause`. Closes SLO-18/59/60/102.

### CG-G [MED] — SLA-tier breach-budget arbiter.
Realtime>Batch exists but premium-vs-bulk arbitration (protect the tightest contract, relegate looser tier within ITS OWN SLA) is unspecified. Add an SLA-tier arbiter to the objective function. IMPL `tier_arbiter_protects_contract_relegates_within_looser_sla`. Closes SLO-82.

- 03_s2s: 50 / 30 / 20  (of 100)  [HARSHEST — V2/IMPL are STT/TTS-framed; ArStep seam is single-stream]

## More cross-cutting gaps (S2S audit)

### CG-H [CRITICAL] — Full-duplex MULTI-STREAM seam (the ArStepModel is single-stream).
`ArStepModel` (IMPL §1/M2.1) = single-stream-in → single-`Frame`-out. Full-duplex S2S needs a `DuplexStepModel`/`MultiStreamSlot` generalization: K-stream interleave (the Moshi 17-stream), `user_in` ALWAYS modeled while speaking + `model_out`, per-stream `(role, delay_sign, ring)`, a `TurnState`/EoT head, a `DoubleTalkPolicy` (backchannel-vs-turn-grab). This is the Moshi 9-item checklist (v1.0 §9) — promote it from M5-prose to a TYPED SEAM in V2 §4 + IMPL M2/M5. Subsumes S2S-3/4/23/24/38/39/43/48/49/50/62/63/69/88/95/96 + the multistream/delay parts of TTS-121/125. Gates: `duplex_user_stream_always_modeled`, `eot_head_emits_confidence`, `double_talk_policy_backchannel_vs_grab`, `multistream_slot_role_delay_sign`.

### CG-I [HIGH] — Cascade-S2S node seams (LLM stream node, sentence aggregator, tool node).
The STT→LLM→TTS DAG is named but has no `LlmStreamNode` (streaming token egress, off the AR clock), no clause-boundary sentence-aggregator (stream first clause to TTS), no off-audio-path tool-call node (partial-fire). Add the three as stage-DAG node types (composes with CG-F reasoning cascade). Closes S2S-2/19/26/27/70.

### CG-J [HIGH] — Milestone the F8 acoustic-delay ring + give StepOutput per-codebook depth.
Catalog F8 (per-codebook delay ring, depth max_delay+2, pad-force warm-up) is precise but the IMPL test-matrix maps F1-F6 and OMITS F8; `StepOutput{frame}` has no K-codebook depth structure (needed for RQ-Transformer depth/MTP). Add F8 gates to M2.3 + `StepOutput` per-codebook shape. Closes S2S-20/79, unblocks S2S-89.

### CG-K [MED] — Masked-slot bandwidth/energy COST charged in admission (asserted, not tested).
R5a/L8 say masked slots aren't free, masked≠absent is gated, but the cost-accounting (the over-admit bug) is not. Add `masked_slot_bandwidth_charged_in_admission` to M4.3. Closes S2S-47. (Note: S2S-87 edge→DC session-spill folds into CG-E fault-migration.)

## EMERGING PATTERN (3 of 10 audits): core is solid, SEAMS+PERIPHERY under-specified. The patch is ADDITIVE — elevate existing-in-codebase/memory pieces (TextFrontend, TransportEgress, REALTIME_REASONING/R6, multistream-delay, full-duplex seam, router) into first-class TYPED SEAMS + GATES; specify the scheduler objective function (risk-EDF); fix the migration self-contradiction. NOT a core reframe.

- 01_stt: 71 / 46 / 3  (of 120)  [feature-surface lives in INFER_SPEC, not folded into V2/IMPL]

## More cross-cutting gaps (STT audit)

### CG-L [CRITICAL] — Fold the FEATURE surface into V2/IMPL as gated nodes (it's in INFER_SPEC, not the design docs).
The engine is specified; the voice FEATURES are not. Add to V2 §4 as typed stage nodes + IMPL gates: **ingress sample-rate normalizer** (8→16k / 44.1→16k anti-alias — the bookend to CG-B TransportEgress; gates `ingress_resample_anti_alias`, `no_chipmunk_on_wrong_sr`); **ASR-feature post-proc** (CTC-collapse, partial-stability/LocalAgreement, `WordTiming` + confidence POPULATION [not just the drift proxy], punctuation/ITN); **language detect/force/code-switch**; **biasing** = a `BiasContext` added to the stepped seam + the F3 `reset_slot` fan-out + folded into the prefix-cache `extra_key` (else cross-tenant bias leak — STT-21); **AED DTW word-alignment** post-node (cross-attention DTW, monotonicity-enforced, lower confidence — whisper has no duration head, STT-22). Closes STT-3/4/8/10/21/22/34 + the bulk of the 46 PARTIALs. (Merges with CG-A TextFrontend on the TTS side → one "feature edges" layer: ingress-normalize/frontend IN, feature-postproc/transport-egress OUT.)

### CG-M [HIGH] — KV is THREE-tier (add StreamingEncoderCache).
R1 made KV two-tier (radix prefix + ring suffix). STT cache-aware streaming encoders (L11, the 560-stream/H100 headline) need a THIRD tier: `StreamingEncoderCache` — bounded delta-state (channel/time caches, sized from genai_config), deltas-only feed. Add to V2 §4 KV model + IMPL milestone. Closes STT-12/70/72/88.

### CG-N [HIGH] — Heterogeneous-placement needs an IMPL MILESTONE (V2 §2.2/§2.3/§3.4 recipes have ZERO gates).
The per-substrate placement + `SharedHostBufType` zero-copy + static-shape-bucket + shared-bandwidth-duty are all in V2 prose but IMPL M5 lists only transport/MTP/duplex. Add an explicit **M4.x/M5.x "heterogeneous placement & zero-copy" milestone** with gates: `stage_placed_on_affinity_substrate`, `shared_host_buftype_zero_copy_on_uma`, `static_shape_bucket_padded_for_npu`, `bandwidth_bound_stages_serialized_on_shared_bus`. Closes STT-35/92/93/94/110 + HW placement scenarios.

### CG-O [META/HIGH] — Close the ORPHAN-MECHANISM class: every V2 mechanism MUST have an IMPL gate.
Recurring across all 4 audits: V2 NAMES a mechanism that IMPL never TESTS (KV-migration-within-slack, intra-node spatial P/D A/B, lazy mode=auto promote, MI300X/multi-model co-residency, cache-aware encoder, heterogeneous placement, reasoning cascade, router). Apply a completeness rule: **add §6 "Coverage completeness" to IMPL — every mechanism in V2 maps to ≥1 named gate; an un-gated mechanism is flagged "requires-gate-before-production."** Generate the V2-mechanism → IMPL-gate cross-table as the attestation.

### Guard merges (dedup): STT-20 whisper hallucination-loop = SAME guard as TTS-95 degeneracy → one `decode_repeat_ngram_guard` covering BOTH STT AR-decode and TTS codec-token loops. STT-73/77/85/96 DC-mechanism gates fold into CG-E (fault-migration) + CG-N (placement) + CG-O (orphan gates).

## STATUS: 4 of 10 (tts 106/19/5, slo 56/38/11, s2s 50/30/20, stt 71/46/3). Awaiting arch/hardware/batching/scaling/failure/features. The fix is a coherent ADDITIVE patch: feature-edges layer (CG-A+L), duplex/multistream seam (CG-H), scheduler objective function + router (CG-C+D), 3-tier KV (CG-M), R6 reasoning (CG-F), heterogeneous-placement+orphan-gate closure (CG-N+O), migration-contradiction fix (CG-E), shared guards (degeneracy, inner-NFE cap, masked-slot charge). Core thesis UNCHANGED.

- 06_batching: 90 / 19 / 5  (of 114)  [CORE SPINE CONFIRMED SOLID]
- 10_features: 22 / 44 / 4  (of 70)   [DAG machinery 0× in engine docs]
- 07_scaling: 48 / 65 / 7  (of 120)   [5 subsystems named-not-specified]

## CONSOLIDATED GAP TAXONOMY (7 of 10 audits) — the core thesis is SATISFIED; gaps are SEAMS/EDGES/DAG/CONTROL/SCHEDULER that V2 named-but-didn't-build. ALL ADDITIVE.

### LAYER 1 — FEATURE EDGES (CG-A + CG-L + features G-FEAT1)
Ingress sample-rate-normalizer → TextFrontend (SSML/locale-TN/ITN/code-switch) → [CORE] → ASR-feature-postproc (CTC-collapse, partial-stability/LocalAgreement, WordTiming+confidence population, AED-DTW alignment, punctuation) / TransportEgress (anti-alias resample, G.711/Opus+FEC, 20ms-RTP repacketize). PLUS non-core `FeatureStage` taxonomy (denoise/dereverb/AGC/VAD/diarize/langID/verify/KWS/neural-SR≠rubato/punct) — each a typed node with a `StageState::reset` contract (NOT just ArStepModel). Biasing = `BiasContext` in the seam + F3 fan-out + prefix `extra_key`.

### LAYER 2 — DAG MACHINERY (features G-DAG1/FINAL1/AGG1/RESET1/CLOUD1) — was in SGLang G11 + v1.0 §3, V2 dropped it
- Dynamic `route_fn`/`wait_for_fn`/`terminals[]`/`JoinByTime` as first-class `StageNode` fields (conditional branch else deadlock) → IMPL M4.1b.
- FINAL is DAG-PROPAGATED: in-band `FINAL{stage_id, after_tail_drain}` on every edge + per-terminal FINAL (else truncation as a stage flushes before its tail drains).
- `SentenceAggregator`/`StableSpanGate` archetype (commit only on sentence boundary → no O(N²) MT churn/flicker).
- DAG-WIDE transactional `DagSlotReset` + DAG-wide `channel_id` (reset_slot is per-AR-model only → cross-user contamination across denoise-gain/MT-context/codec-window).
- `CloudStage{paradigm=remote}` (vendor-mixed: network budget, credit-relay to remote, fail-fast-on-disconnect, remote barge-in cancel, terminal-Error fan-out).
- DAG-wide barge-in: `barge_in_fans_out_to_all_stages_with_ack` (G9 reliable, not fire-and-forget).

### LAYER 3 — DUPLEX/MULTISTREAM SEAM (CG-H + CG-J) [already captured — reconfirmed by batching BAT-44]
ArStep → DuplexStepModel/MultiStreamSlot; StepOutput per-codebook depth; **acoustic-delay ring F8 moves to M2 (not M5)** — it's a correctness prereq for Moshi/Mimi/Orpheus multi-codebook TTS the plan headlines (`acoustic_delay.rs`, depth max_delay+2, pad-force warm-up).

### LAYER 4 — SCHEDULER OBJECTIVE FUNCTION + ROUTER + TIERS (CG-C + CG-D + CG-G + batching binding-resource)
Computable objective: maximize Σviable s.t. ΣU≤bound ∧ Σbw≤ceiling, **risk-EDF** ordering, shed by criticality+age. **`bottleneck = argmax_r utilization(r)` over {compute×N-substrates, shared-bandwidth}, re-picked per admit** (the mixed-clock 64-AR+8-CFM+codec+STT feasibility — BAT-105/107/114). Prefix-affinity ROUTER (CG-D). Per-SLO-tier RESERVED duty (admit gold preferentially — SCALE-57/76/96).

### LAYER 5 — CONTROL PLANE + LIFECYCLE + CROSS-CELL (scaling G-CTRL/FSM/XLEDGER/PROMOTE/MIGRATE + CG-E)
- **Control-plane contract** (the engine↔orchestrator line): drain/load/unload/lifecycle/reject-reason/used-total-slots API + autoscale/warm-pool/spill-routing/rollout-sequencing/canary-routing/region-failover/`freeze-rollout` + `reconnect_admission_rate_capped_per_replica`. → V2 §3.10 + IMPL M4.5. WaaV only EMITS used/total today.
- **Per-replica lifecycle FSM** Loading→Warming→Ready⇄Degraded→Draining→Failed (`lifecycle.rs`, transition gates; Degraded-lowers-ceiling, Draining-on-refcount-zero). Distinct from `marker.rs` (per-stream).
- **Box-scoped SINGLETON VRAM accountant** — REAL CORRECTNESS GAP: cell/shard gives each cell its own CUDA context → two cells each "see" free VRAM → double-load OOM. Serialize all cross-cell loads; gate `two_concurrent_cross_cell_loads_serialize`.
- **Config-tier auto-promotion** live executor-swap without dropping the stream: gate `second_stream_promotes_without_dropping_first`.
- **Fault-migration protocol** (CG-E): default no-migration (cadence=buffer); fault/spill = opt-in, same-version, LEASED ownership, mid-migration-abort, zombie-slot + split-brain guards.

### LAYER 6 — R6 REASONING CASCADE (CG-F + CG-I) [captured]
latency-filler + sentence-agg + two-tier + barge-in-cancels-LLM + `LlmStreamNode`/tool-node.

### LAYER 7 — GUARDS + KV-TIER + META (CG-K/M/N/O + batching guards)
3-tier KV (+`StreamingEncoderCache` CG-M); heterogeneous-placement IMPL milestone (CG-N); masked-idle **compaction.rs OR charge-at-captured-count** (BAT-75/76/111 — pick ONE, not just assert); cycle-safe sniffer gate (BAT-67, WaaV's prior scar); relay credit back-pressure + notify-before-wait (BAT-52); decode-degeneracy n-gram guard (STT-20=TTS-95, ONE guard); per-slot inner-NFE cap (TTS-106); mid-tick in-flight-recycle guard (BAT-106); streaming≠non-streaming micro-batch + pre-payload opt-in (BAT-42/54). **META (CG-O): IMPL §6 coverage-completeness — every V2 mechanism → ≥1 named gate; un-gated = "requires-gate-before-production."**

## STATUS: 7 of 10. SATISFIED-rate by family: batching 79%, stt 59%, tts 82%, slo 53%, s2s 50%, scaling 40%, features 31%. Pattern: core-engine families high, feature/DAG/scaling/control families low. Awaiting arch/hardware/failure (expect arch+failure high [core], hardware med [placement gates]).

- 09_failure: 86 / 21 / 11  (of 118)  [composed-cascade holds; 4 v1.0 mechanisms dropped in reframe]

## More gaps (failure audit) + CROSS-DOC HYGIENE

### LAYER 4/5 RESTORE — v1.0 §6 mechanisms DROPPED in the V2 reframe (real regressions to restore):
- **Thermal/DRIFT-RESPONSE entirely absent** (FAIL-23/78/110/113): no live-measured-step-time admission input, no drift-response. RESTORE: EWMA measured-step-time into R3/M4.3 + `sustained_p99_breach_trips_drift_response_with_hysteresis` (60s hysteresis, shed-newest-least-progressed). Breaks 2 EXTREME cascades.
- **Calibration-stamp lifecycle absent** (FAIL-93/94/113): no persistence; a driver bump / MIG-repartition silently over-admits. RESTORE: `device+driver+warm-set` stamp gates /readyz + `admission_refuses_stale_stamp` (M4.4) + `calibration_stamp_cache_hit_skips_recalibration` (SCALE-45/94 — fast rollback enabler).

### LAYER 7 additions (failure):
- **FAIL-99 q4f16-on-CUDA empty-KV dtype seam** (WaaV's documented voxtral fix): graph-driven dtype via `StaticGraph::input_types()` — the "zero-code weight swap" crashes without it. Add to backend-api + gate `empty_kv_dtype_follows_weight_precision_q4f16`.
- **Teardown ORDERING ungated** (FAIL-87/108): `teardown_aborts_collectives_before_destroy` + timed kill-ladder + `restart_waits_on_vram_reclamation` (the NCCL-destroy-hang root H7).
- **zero-D2H-sync-in-decode** stated but untested (FAIL-8/101): explicit `zero_d2h_sync_during_decode` gate (the whole 9ms/step budget rests on it).
- shm orphan-reaper (FAIL-49, LAYER7 relay).

### CROSS-DOC HYGIENE (fix in the patch):
- **HG-1: v1.0→v2 §-CROSSWALK.** ~25 scenarios cite v1.0 §3.4/§4.5/§5.1/§5.2/§6 that don't exist in V2. Add a crosswalk table to V2; where a mechanism didn't survive the reframe (thermal/drift, calibration, parts of HAL/stage-DAG/precision), RESTORE it. (This is the root of many PARTIALs — V2 condensed v1.0 and orphaned its cross-references.)
- **HG-2: sm120 vs sm121.** V2 header says sm121; catalog + all scenarios + the cited vLLM graph-hang scars + the eager-fallback gate say sm120. Reconcile to ONE device-arch string (GB10 = sm121 per nvidia, but the vLLM scars are filed against sm120-class Blackwell; state both: "GB10 sm_121, in the sm_12x Blackwell family the vLLM graph-hang scars target").

## STATUS: 8 of 10 (awaiting arch, hardware). The patch plan is FROZEN as the 7-layer taxonomy + 2 hygiene fixes + 4 v1.0-restore items. Core thesis UNCHANGED & SATISFIED; everything is additive specification of named-but-unbuilt seams/edges/DAG/control/scheduler-function + restoration of 4 dropped v1.0 mechanisms.

- 05_hardware: 32 / 79 / 10  (of 121)  [HAL half is prose-complete in v1.0 but TEST-GATE-EMPTY in IMPL]

## More gaps (hardware audit) — fold into LAYER 7 (heterogeneous placement) + LAYER 1 + precision

### LAYER 7 — StagePlacer IMPL milestone (CG-N, now quantified as an 11-test block):
- **`StagePlacer` + zero-copy relay never built** (20 PARTIALs): §3.4 ggml decision-order + `ZeroCopyBuffer` (alias-on-UMA / discrete-copy / per-edge-relay / cycle-safe-sniffer) = prose; M4.x ships only a `substrate` FIELD. Add the 11-test StagePlacer gate block.
- **HW-31 degrade-to-CPU**: `ep_fault_degrades_op_to_forward_native_with_telemetry` (P-6 floor, op-level not just crash).
- **Shared-bandwidth needs a per-stage `roofline_class`** (HW-52/54/55/60/62/76/81/82/103): classify {compute-bound | bandwidth-bound}; serialize two bandwidth-bound, overlap compute∥bandwidth; `Σbw≤S·ceiling` named gate.
- **HW-15 ridge-point batch-knee** (keystone): `per_substrate_batch_knee_from_ridge_point` (peak-compute÷peak-bandwidth — a-priori, distinct from the measured duty ledger which underfills B200).
- **HW-99/100**: `boot_reserves_graph_pool_delta` + `warmup_gates_readiness` (the #44209 ready-then-crash-loop sm120 scar) as discrete gates.

### Precision resolution (highest-value, GPU-free in the NopLoader idiom):
- **`int8_file_never_lands_on_ort_cuda_ep`** + `precision_resolves_per_active_ep` (the §5.2 master-constraint 12ms→232ms; `by_substrate[ep]`). Closes 7 HW scenarios. GPU-FREE unit tests in `model.rs`.
- **`EpKind::Hpu`** (Gaudi): add the enum + a §2.2 Gaudi row (systolic-MME → WIDE batch, HBM — the generic NPU row wrongly models it as static-B=1) + `hpu_degrades_to_forward_native`.
- **Per-substrate accuracy/MOS stamp** (HW-32/33/18): `quant_variant_gated_by_per_substrate_accuracy_stamp` INCLUDING the TTS MOS check (WER-flat/MOS-crash signature).
- **HW-5/10**: `slot_cap_by_vram_capacity` (RTX/GDDR), `multi_model_co_residency_admission` (MI300X 192GB).

### LAYER 1 (hardware-confirmed):
- HW-35/36: `egress_downsample_to_8k_is_anti_aliased` + `codec_and_resample_run_off_ar_clock` (= TransportEgress LAYER1).

## STATUS: 9 of 10 (awaiting ONLY 04_arch — a core-paradigm family, expect high-SATISFIED). PATCH TAXONOMY FROZEN: 7 layers + 2 hygiene + 4 v1.0-restore + HAL-gate-block. Aggregate SATISFIED so far ≈ 545/988 raw, but ~90% of NON-satisfied are PARTIAL "named-not-gated" (mechanism present, gate missing) → closeable by ADDING gates/specs, not redesign. TRUE GAPs (no mechanism anywhere) ≈ 76/988 ≈ 8%, all additive. Core thesis intact.

- 04_arch: 90 / 25 / 0  (of 115)  [nested/3rd-class/cohort core REAL; 25 underspecified, 0 holes]

## FINAL gaps (arch audit) — the third-class CORRECTNESS specifics
- **ADMISSION MATH BUG**: `T_step = T_ar + inner_steps×T_inner` assumes SCALAR NFE → WRONG under per-stream variable NFE. FIX: `T_step = T_ar + max_over_active(inner_steps_i × T_inner)` + gate `nested_variable_nfe_T_step_sums_max_subbucket`.
- **Positive sub-bucket-by-NFE composition** unspecified (only negative tested): `sub_bucket_inner_by_nfe` data structure (group B hidden-states into per-NFE inner passes within one outer tick, reassemble in slot order) + gate `nested_sub_buckets_by_nfe_within_one_outer_step`.
- **Bandwidth-duty MEASUREMENT method** (the concrete spec the ledger needs): `bandwidth_duty = bytes_touched/ceiling × tick_rate` via DRAM_ACTIVE during CO-LOAD calibration (calibration currently measures T_step compute only). → resolves the bandwidth-ledger gate flagged by slo/hardware/failure/arch.
- **Masked-slot cost — DECIDE the disjunction** (R5a "compact OR budget" has no algorithm): pick the `masked_bandwidth_duty` admission term (simpler, KISS) as default + optional repack-trigger; gate it.
- **Inner-solver state in co-eviction + migration** (privacy): F3 `reset_slot` fan-out must enumerate depth-decoder/inner-CFM latent; migration must move it. Gates `triple_nested_reset_slot_fans_out_to_inner_solver_state`, `kv_migration_moves_inner_solver_state`.
- **Patch-stride derivation**: `samples_per_patch`/`patch_period` + dynamic-FR drain-to-tick-boundary regroup (no frame-drop across a stride change).
- **Per-model realtime FEASIBILITY reject**: `min_step(B=1) ≤ T_f` gate `reject_model_when_min_step_exceeds_frame_period` (nothing refuses a 150Hz/75Hz-nested model that can't be realtime even at B=1).
- **Path-B sidecar slot-keyed crosstalk gate** (C3 demanded): `sidecar_state_slot_keyed_no_crosstalk` (M3.1 tests only Rust egress crosstalk, not the Python sidecar codec/sliding-window state).
- Doc note: v1.0 §-refs DO resolve (non-breaking space) → HG-1 softens: v1.0 is governing-substrate; the real issue = (a) RESTORE genuinely-dropped v1.0 mechanisms (thermal/drift §6, calibration), (b) ADD gates for v1.0-substrate mechanisms (HAL §2, stage-DAG §3), (c) add a crosswalk.

## ============ FINAL AGGREGATE (all 10) ============
## SATISFIED 651 (58.5%) · PARTIAL 386 (34.7%) · GAP 76 (6.8%) · total 1113
## Per family SAT%: arch 78, batching 79, tts 82, stt 59, failure 73, slo 53, s2s 50, scaling 40, hardware 26, features 31
## VERDICT: core thesis SATISFIED & unchanged; ALL non-SAT are ADDITIVE (PARTIAL=add-a-gate/spec, GAP=add-a-mechanism). NO reframe. Patch = 7 layers + 2 hygiene + 4 v1.0-restore + the arch correctness fixes → every scenario gets a mechanism + a gate.
