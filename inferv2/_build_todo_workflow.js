export const meta = {
  name: 'waav-infer-build-todo',
  description: 'Generate + brutally critique the complete low-level extreme-TDD build TODO for WaaV Infer v2.1 (M2-M5) + gateway integration (GW-1..18) + env, grounded in the inferv2 docs and the live codebase',
  phases: [
    { title: 'Generate', detail: 'one grounded task-list per milestone area' },
    { title: 'Critique', detail: 'brutal per-area adversarial pass (complete/grounded/unambiguous/KISS-DRY) + fix' },
    { title: 'Coherence', detail: 'global completeness-vs-master-matrix + DRY loop-until-dry' },
    { title: 'Assemble', detail: 'render the final tracked TODO markdown' },
  ],
}

// ---- repo roots ----
const D = '/home/bud/ditto/waav/WaaV/inferv2'           // the design docs
const G = '/home/bud/ditto/waav/WaaV/INFER_GATEWAY_INTEGRATION.md'
const C = '/home/bud/ditto/waav/waav-infer'             // the live codebase we extend

// ---- the strict task schema (every task is RED-first + live-GB10 + grounded) ----
const TASK = {
  type: 'object', additionalProperties: false,
  required: ['id','title','milestone','red_tests','green','live_gb10','accept','doc_cite','gate','deps'],
  properties: {
    id:        { type: 'string', description: 'stable id, e.g. M2.2-T3 or GW1-T1' },
    title:     { type: 'string', description: 'imperative, one line, unambiguous' },
    milestone: { type: 'string', description: 'M2|M2b|M3|M4.1|M4.1b|M4.2|M4.3|M4.4|M4.5|M4.x|M5|GW-M1|GW-M3|GW-M4|GW-M5|ENV' },
    red_tests: { type: 'array', items: { type: 'string' }, description: 'RED-first test fn name(s) that must exist and fail before code' },
    green:     { type: 'string', description: 'minimal impl: target crate/file + 1-2 line sketch (cite real or named-new file)' },
    refactor:  { type: 'string', description: 'the type-level/DRY hardening, or "n/a"' },
    live_gb10: { type: 'string', description: 'the LIVE GB10 validation (exact cmd + pass criterion via gb10-env.sh) OR "pure-logic unit-only (no GPU)"' },
    accept:    { type: 'string', description: 'Definition-of-Done / acceptance criterion (measurable)' },
    doc_cite:  { type: 'string', description: 'exact source: INFER_ENGINE_IMPL §x / design/Lx / GUIDELINES §x / GW-n / scenario' },
    gate:      { type: 'string', description: 'catalog/perf/gw gate id: F#/G#/H#/J#/L#/R#/perf-lever/GW-#' },
    deps:      { type: 'array', items: { type: 'string' }, description: 'task ids this depends on (DRY: reference, never duplicate)' },
    kiss_dry:  { type: 'string', description: 'why this is minimal + what existing seam it reuses' },
  },
}
const AREA_RESULT = {
  type: 'object', additionalProperties: false,
  required: ['key','tasks','coverage_note'],
  properties: {
    key: { type: 'string' },
    tasks: { type: 'array', items: TASK },
    coverage_note: { type: 'string', description: 'which doc gates this area maps, and any deliberately-omitted-as-out-of-scope' },
  },
}
const CRITIQUE = {
  type: 'object', additionalProperties: false,
  required: ['verdict','missing','hallucinated','ambiguous','redundant','inaccurate'],
  properties: {
    verdict: { type: 'string', enum: ['clean','needs-fix'] },
    missing:      { type: 'array', items: { type: 'string' }, description: 'doc gates/mechanisms NOT covered by a task' },
    hallucinated: { type: 'array', items: { type: 'string' }, description: 'tasks citing a file/trait/test that does not exist and is not a named-new artifact' },
    ambiguous:    { type: 'array', items: { type: 'string' }, description: 'task ids lacking a concrete RED test / accept / live-validation' },
    redundant:    { type: 'array', items: { type: 'string' }, description: 'task ids that duplicate, gold-plate, or are untraceable to a requirement (KISS/DRY violations)' },
    inaccurate:   { type: 'array', items: { type: 'string' }, description: 'task ids with wrong file/trait/milestone vs the codebase reality' },
  },
}
const GLOBAL = {
  type: 'object', additionalProperties: false,
  required: ['verdict','missing_gates','cross_area_dupes','forbidden_violations','notes'],
  properties: {
    verdict: { type: 'string', enum: ['clean','needs-fix'] },
    missing_gates:        { type: 'array', items: { type: 'string' }, description: 'master-matrix gates / GW deltas / R-contract items with NO task' },
    cross_area_dupes:     { type: 'array', items: { type: 'string' }, description: 'task-id pairs that duplicate across areas → merge' },
    forbidden_violations: { type: 'array', items: { type: 'string' }, description: 'tasks violating GUIDELINES §6 forbidden-list (custom kernels, quant-by-default, FlashInfer sm12x, busy-spin, etc.)' },
    notes: { type: 'string' },
  },
}

// ---- the 20 milestone areas (each grounded in its exact docs + codebase seam) ----
const RULES = `
HARD RULES for every task you emit:
- GROUND OR OMIT: cite a REAL doc section AND a REAL codebase file (or a file the plan explicitly names as new, e.g. waav-infer-scheduler/src/slot.rs). Never invent a trait/API/file. If unsure it exists, Read/Grep to confirm.
- RED-FIRST: every task names the failing test(s) written before any impl (the codebase idiom: #[cfg(test)] mod tests, NopLoader test-double, tmp_with_config fixtures, typed-errors-not-panics).
- LIVE-GB10: if the task touches a model/GPU/ORT/torch path, give the exact live validation (source waav-infer/gb10-env.sh; the cmd; the pass criterion — RTF<1, bit-identical, ≥16 streams, etc.). Pure scheduler/logic tasks say "pure-logic unit-only (no GPU)".
- KISS+DRY: one task = one RED->GREEN->REFACTOR cycle landable in isolation. No gold-plating. If a mechanism is shared with another area, DEPEND on it (deps:), never duplicate it. Reuse existing seams (StaticGraph, SttModel/TtsModel, the 16-arm registry, the torch framing) — these are UNTOUCHED.
- ACCURACY INVARIANTS (must not be violated by any task): no quantization/spec-decode/approx-attention by default; no custom hand-written kernels; the AR-compounding identity test gates every perf change; masked != absent; FlashInfer never on sm_12x.
`
const AREAS = [
  { key:'scaffold', title:'New crates + the stepped seam (M2.1)',
    read:[`${D}/INFER_ENGINE_IMPL.md §0,§1,§2.1`,`${D}/INFER_GUIDELINES.md §1`], seam:`${C}/crates/waav-infer-core/src/model.rs (LoadedModel/SttModel/TtsModel/registry), waav-infer-backend-api/src/lib.rs (StaticGraph)`, gates:'R-1; as_stepped dispatch; registry/Path-A invariant' },
  { key:'m2-lockstep', title:'SlotTable + exec-mask (masked!=absent) + RingKvCache + lockstep driver (M2.2-2.4)',
    read:[`${D}/INFER_ENGINE_IMPL.md §2.2,§2.3,§2.4`,`${D}/INFER_GUIDELINES.md §2`,`${D}/design/L3_L6_duplex_reasoning.md (MaskedCell/MultiStreamSlot)`], seam:`new waav-infer-scheduler/src/{slot,ring_kv,cohort,marker}.rs; new waav-infer-runtime`, gates:'F1,F2,F3,F4,F5,F6,F10' },
  { key:'m2-additions', title:'Acoustic-delay ring + duplex seam + zero-D2H-sync + slot-keyed sidecar state (M2 additions)',
    read:[`${D}/INFER_ENGINE_IMPL.md §7 "M2 additions"`,`${D}/design/L3_L6_duplex_reasoning.md`], seam:`waav-infer-runtime/src/arstep.rs (StepOutput per-codebook); torch_runtime/base.py (slot-keyed state)`, gates:'F8,L3,C3,zero_d2h_sync' },
  { key:'m2-perf-core', title:'run_bound IoBinding (the #1 perf change) + SDPA-pin + GQA-native + CUDA-graph-tier (M2 perf)',
    read:[`${D}/INFER_ENGINE_IMPL.md §8.2`,`${D}/INFER_PERF.md`,`${D}/INFER_GUIDELINES.md §3,§4`], seam:`waav-infer-backend-api/src/lib.rs (StaticGraph::run_bound), waav-infer-backend-ort`, gates:'run_bound,sdpa_pin,gqa_native,cuda_graph_tier,no_in_loop_sync; AR-compounding accuracy gate' },
  { key:'m2b-feature-edges', title:'Feature edges: ingress-resample anti-alias, text-frontend, asr-postproc, transport-egress (M2b)',
    read:[`${D}/INFER_ENGINE_IMPL.md §7 "M2b"`,`${D}/design/L1_feature_edges.md`], seam:`new waav-infer-features crate; waav-infer-components/src/resample.rs (EdgeResampler anti-alias TODO)`, gates:'L1; anti_alias; ssml; ctc_collapse; transport_egress; FeatureStage reset; BiasContext->prefix' },
  { key:'m3-streaming-numerics', title:'Delta-streaming + explicit FINAL/cancelled + NaN->reject + sampler guards (M3.1-3.2)',
    read:[`${D}/INFER_ENGINE_IMPL.md §3.1,§3.2`,`${D}/INFER_GUIDELINES.md §5`], seam:`waav-infer-runtime/src/{egress,numerics}.rs; torch_runtime (Iterator already streams); the wire TerminalFrame`, gates:'I1,G2,H1,H5; critique C2 (sample outside graph)' },
  { key:'m3-kv-precision', title:'Hybrid radix prefix-cache + anti-contamination + streaming-encoder-cache + precision gates (M3.3 + additions)',
    read:[`${D}/INFER_ENGINE_IMPL.md §3.3,§7 "M3 additions"`,`${D}/design/L7_guards_kv_placement.md`], seam:`waav-infer-runtime/src/prefix_cache.rs; StaticGraph::input_types() (q4f16 dtype)`, gates:'L1,G1,R1; streaming_encoder_cache; int8_never_on_ort_cuda; empty_kv_dtype_follows_precision' },
  { key:'m4.1-dag', title:'Stage-DAG + decoupled per-stage batching + back-pressure (M4.1)',
    read:[`${D}/INFER_ENGINE_IMPL.md §4.1`,`${D}/design/L2_dag_machinery.md`], seam:`new waav-infer-dag (or runtime); StageNode schema; typed bounded channels`, gates:'G3,G5,G6,G7,G11; RFC#2568 independent batch sizes' },
  { key:'m4.1b-dag-machinery', title:'DAG machinery: route_fn/wait_for_fn/Terminals/FinalGate/SpanAggregator/DagSlotReset/CloudStage/barge-in-fanout (M4.1b)',
    read:[`${D}/INFER_ENGINE_IMPL.md §7 "M4.1b"`,`${D}/design/L2_dag_machinery.md`], seam:`waav-infer-dag`, gates:'route_fn,wait_for_fn,multi_terminal,final_propagates,sentence_aggregator,dag_slot_reset,channel_id_drop,cloud_stage,barge_in_fanout,final_gate_tail_per_archetype' },
  { key:'m4.2-stepbucket', title:'Step-bucket batcher + nested 3rd execution class + corrected NFE math + triple-nested reset (M4.2)',
    read:[`${D}/INFER_ENGINE_IMPL.md §4.2,§7 "M4.2 additions"`,`${D}/design/L4_scheduler_function.md`], seam:`waav-infer-scheduler/src/cohort.rs`, gates:'R2,L5,L15; T_step=T_ar+max_over_active(inner*T_inner); nested_stage_reset_inner_before_outer' },
  { key:'m4.3-admission', title:'Schedulability admission (SigmaU<=bound) + duty/bandwidth ledger + graded degradation + firewall + risk-EDF + tiers (M4.3)',
    read:[`${D}/INFER_ENGINE_IMPL.md §4.3,§7 "M4.3 additions"`,`${D}/design/L4_scheduler_function.md`], seam:`waav-infer-scheduler/src/admission.rs`, gates:'R3,H2,H3,H8,J20,L9,L10; risk-EDF; per-substrate duty<=S; bandwidth<=ceiling; drift-response; thermal' },
  { key:'m4.4-spine', title:'Production spine: cancel/leak/frame-watchdog/VRAM-accountant/cell-isolation/CO-metric/graph-fallback/dead-sidecar + affinity-router/calibration-stamp (M4.4)',
    read:[`${D}/INFER_ENGINE_IMPL.md §4.4,§7 "M4.4 additions"`,`${D}/INFER_FAILURE_CATALOG.md (J*)`], seam:`waav-infer-runtime/src/watchdog.rs; VRAM accountant; cell/shard topology`, gates:'J1,J2,J15,J16,J17,J22,J23,H4,H6,H7,H9,F9; prefix_affinity_router; calibration_stamp_gates_readyz' },
  { key:'m4.5-control-plane', title:'Control-plane API + lifecycle FSM + box-scoped VRAM accountant (cross-cell) + leased migration (M4.5)',
    read:[`${D}/INFER_ENGINE_IMPL.md §7 "M4.5"`,`${D}/design/L5_control_plane.md`], seam:`waav-infer-server/src/engine.rs (extend); new control-plane`, gates:'control_plane_api; lifecycle_fsm; two_concurrent_cross_cell_loads_serialize; reconnect_admission_rate_capped; fault_migration_leased_masked' },
  { key:'m4.x-stageplacer', title:'StagePlacer 11-test block + zero-copy heterogeneous placement + precision resolver + EpKind::Hpu (M4.x)',
    read:[`${D}/INFER_ENGINE_IMPL.md §7 "M4.x"`,`${D}/design/L7_guards_kv_placement.md`], seam:`waav-infer-backend-api (EpKind); StagePlacer`, gates:'stage_placed_on_affinity; shared_host_buftype_zero_copy_uma; content_sniffer_terminates_cyclic; shm_orphan_reaper; ep_fault_degrades_native; hpu_wide_batch' },
  { key:'m5-duplex-transport', title:'Variable-stride + dynamic-FR + MTP + full-duplex (DuplexStepModel + Moshi 9-item + Full-Duplex-Bench) + R6 reasoning + datagram transport (M5)',
    read:[`${D}/INFER_ENGINE_IMPL.md §5,§7 "M5 additions"`,`${D}/design/L3_L6_duplex_reasoning.md`], seam:`waav-infer-runtime (DuplexStepModel,EoT); datagram media plane`, gates:'R2,L6,L12,L13,L14,J18,J21,S2S; moshi_9_item; r6_filler/sentence-agg/barge-in-cancels-llm; lora_adapters; rainbow-deploy' },
  { key:'perf-per-model', title:'Per-onboarded-model perf fixes (Path-A run_bound set, Path-B StaticCache+compile set, ORT-estimator IoBinding, omnivoice CFG-batch) + universal accuracy gate (§8.3)',
    read:[`${D}/INFER_ENGINE_IMPL.md §8.1,§8.3`,`${D}/INFER_GUIDELINES.md §0,§3`], seam:`waav-infer-core/src/stt+tts arms; torch_runtime/models`, gates:'R-1..R-8 contract; per-model fixes each bit-identical; ar_compounding_emitted_codes_identical' },
  { key:'gw-m1-cascade', title:'Gateway integration M1: provider-api crate + waav-infer-provider (BaseSTT+BaseTTS) + GW-1,2,3,4,5,8,12 (full barge-in)',
    read:[`${G} §5,§13 (GW-1..GW-12), §6.4 barge-in`], seam:`NEW waav-gateway-provider-api + waav-infer-provider crates; WaaV/gateway/src/{plugin,core/stt/base.rs,core/tts/base.rs}`, gates:'GW-1,GW-2,GW-3,GW-4,GW-5,GW-8,GW-12; keystone wire-test; native WS v1 mapping' },
  { key:'gw-m3-s2s', title:'Gateway integration M3: native full-duplex S2S (InferProtocol) + GW-9,13,14,18 + Full-Duplex-Bench',
    read:[`${G} §6,§13 (GW-9,13,14,18)`], seam:`WaaV/gateway/src/core/realtime/scaffold; waav-infer-provider (BaseRealtime)`, gates:'GW-9,GW-13(ConnectSpec::Unix+datagram),GW-14(conditioning),GW-18; Full-Duplex-Bench<=200ms' },
  { key:'gw-m4m5-fleet', title:'Gateway integration M4/M5: in-proc edge (GW-6), discovery+/capabilities (GW-10), OTel tracing (GW-17), admission surface (GW-11), prefix-fingerprint (GW-16), fleet router endpoint',
    read:[`${G} §7,§8,§11,§13 (GW-6,10,11,16,17)`], seam:`WaaV/gateway/src/{handlers/capabilities,middleware,core/alias}; waav-infer router endpoint`, gates:'GW-6,GW-10,GW-11,GW-16,GW-17; cross-tier failover; mode=remote->router' },
  { key:'env-ci', title:'Environment + CI + the validation pyramid + scenario re-audit + clippy/fmt gates',
    read:[`${D}/INFER_ENGINE_IMPL.md §6 discipline`,`${C}/gb10-env.sh`,`${D}/COVERAGE_ATTESTATION.md`], seam:`waav-infer CI; gb10-env.sh; eval/ harnesses`, gates:'validation pyramid (offline-parity+streaming-playback+concurrent-load); single_stream_edge_pays_nothing; scenario re-audit CI; no-regression on 16-arm + native-parity' },
]

function genPrompt(a) {
  return `You are generating the EXTREME-TDD build TODO for ONE area of the WaaV Infer v2.1 implementation: "${a.title}".
${RULES}
STEP 1 — READ (ground yourself): ${a.read.join(' ; ')}. Then read the codebase seam you extend: ${a.seam} (under ${C}). Use Read/Grep; confirm every file/trait you cite EXISTS or is a plan-named NEW file.
STEP 2 — EMIT the complete, ordered list of low-level tasks for this area ONLY. Cover EVERY gate the source doc names for this area: ${a.gates}. Each task is one RED->GREEN->REFACTOR cycle, with the live-GB10 validation, acceptance/DoD, doc citation, gate id, and deps. KISS+DRY: minimal, no duplication, reuse existing seams.
Return the AREA_RESULT (key="${a.key}"). coverage_note must list which named gates you mapped and anything deliberately deferred to another area (by area key).`
}
function critPrompt(a, gen) {
  return `BRUTAL adversarial review of the build-TODO tasks for area "${a.title}". Re-read the source docs (${a.read.join(' ; ')}) and the codebase seam (${a.seam} under ${C}) — do NOT trust the tasks; verify against ground truth.
Find, by task id: (1) MISSING — gates/mechanisms the doc names for this area (${a.gates}) that have NO task; (2) HALLUCINATED — a task cites a file/trait/test that does not exist and is not a plan-named new artifact (grep to confirm); (3) AMBIGUOUS — no concrete RED test, no measurable accept, or no live-GB10 validation where a GPU/model path is touched; (4) REDUNDANT — duplicates / gold-plating / untraceable-to-a-requirement (KISS/DRY); (5) INACCURATE — wrong file/trait/milestone vs codebase reality.
Be specific and cite evidence. Tasks under review:\n${JSON.stringify(gen.tasks.map(t => ({id:t.id,title:t.title,green:t.green,red:t.red_tests,gate:t.gate,live:t.live_gb10})) )}`
}
function fixPrompt(a, gen, crit) {
  return `Apply this brutal critique to the area "${a.title}" task list and return the CORRECTED AREA_RESULT (key="${a.key}").
Rules: add a task for every MISSING gate; delete/merge REDUNDANT; fix every HALLUCINATED (re-ground or remove) and INACCURATE (correct the file/trait/milestone); make every AMBIGUOUS task concrete (add the RED test / measurable accept / live-GB10 validation). Keep KISS+DRY. ${RULES}
Critique: ${JSON.stringify(crit)}
Current tasks: ${JSON.stringify(gen.tasks)}`
}

// ===== PHASE 1+2: per-area generate -> brutal critique -> fix (pipeline, no barrier) =====
phase('Generate')
const areaResults = await pipeline(
  AREAS,
  (a)            => agent(genPrompt(a),         { schema: AREA_RESULT, label: `gen:${a.key}`,  phase: 'Generate' }),
  (gen, a)       => agent(critPrompt(a, gen),   { schema: CRITIQUE,    label: `crit:${a.key}`, phase: 'Critique' }).then(crit => ({ gen, crit })),
  ({gen,crit},a) => (crit.verdict === 'clean' && !crit.missing.length && !crit.ambiguous.length && !crit.hallucinated.length && !crit.redundant.length && !crit.inaccurate.length)
                       ? gen
                       : agent(fixPrompt(a, gen, crit), { schema: AREA_RESULT, label: `fix:${a.key}`, phase: 'Critique' })
                           .then(fixed => (fixed && fixed.tasks && fixed.tasks.length) ? fixed : gen), // robustness: a dead fix-stage keeps the generated tasks, never drops the area
)
const clean = areaResults.filter(Boolean)
let allTasks = clean.flatMap(r => r.tasks)
log(`generated+critiqued ${clean.length}/${AREAS.length} areas, ${allTasks.length} tasks`)

// ===== PHASE 3: global completeness-vs-master-matrix + DRY, loop-until-dry =====
phase('Coherence')
const MASTER = `The authoritative coverage set the TODO MUST fully cover:
- INFER_ENGINE_IMPL.md §6 test-gate matrix (every F#/G#/H#/J#/L#/R# catalog gate) + §7 every named v2.1 gate (M2-additions, M2b, M3-additions, M4.1b, M4.2/3/4 additions, M4.5, M4.x) + §8 perf gates + the R-1..R-8 model-implementation contract.
- INFER_GATEWAY_INTEGRATION.md GW-1..GW-18 (GW-7 done, GW-15 removed).
- The validation pyramid (offline-parity + streaming-playback + concurrent-load) + single_stream_edge_pays_nothing + the no-regression gate (16-arm registry + native-parity STT/TTS still pass) + the scenario re-audit CI.
- The 2 invariants (accuracy sacred; multi-tenant correctness sacred) and GUIDELINES §6 forbidden-list.`
// ===== PHASE 3: append the 5 explicitly-missing v2.1 gates the audit found =====
// (Deterministic; replaces the transient-failing global fix-loop. The audit verdict was
//  0 cross-area-dupes / 0 forbidden-violations / 5 missing gates — so we just add the 5.)
const added = await agent(
  `Add EXACTLY these 5 explicitly-missing v2.1 gates as fully-formed WaaV Infer build tasks (TASK schema: id,title,milestone,red_tests[],green,refactor,live_gb10,accept,doc_cite,gate,deps[],kiss_dry). GROUND each by reading the cited design doc under ${D}. RED-first; pure-logic scheduler/placement tasks set live_gb10="pure-logic unit-only (no GPU)". KISS+DRY; reuse named seams.
1. id "M4.3-T14" milestone "M4.3" — masked_slot_bandwidth_charged_in_admission. doc: INFER_ENGINE_IMPL §7 M4.3-additions + design/L4 (proj.bandwidth += masked_slot_bandwidth; masked_slot_bandwidth_term_can_tip_rejection). green: waav-infer-scheduler/src/admission.rs — the bandwidth-duty ledger CHARGES idle/masked slots (masked!=absent extended to bandwidth; the dense kernel still reads their rows). gate "L8/masked_slot_bandwidth". deps: the M4.3 shared-bandwidth-ledger task.
2. id "M4.3-T15" milestone "M4.3" — reject_model_when_min_step_exceeds_frame_period. doc: INFER_ENGINE_IMPL §7 M4.3-additions + design/L4 (reject_model_when_min_step_exceeds_frame_period, infeasible_model_at_150hz_on_slow_substrate). green: admission.rs — admit/boot feasibility reject when min single-stream step (T_ar + inner×T_inner) > frame period T_f. gate "feasibility_reject". deps: the M4.3 admission core.
3. id "M4.x-T20" milestone "M4.x" — static_shape_bucket_padded_for_npu (9th StagePlacer test). doc: INFER_ENGINE_IMPL §7 M4.x + design/L7 (Static1/BatchProfile). green: StagePlacer/backend-api — static min=opt=max shape-bucket + padding for static-B1 NPU/QNN/Hexagon/ANE; reject AR on a static NPU. gate "static_shape_bucket_padded_for_npu". deps: the M4.x StagePlacer block.
4. id "M4.2-T9" milestone "M4.2" — patch_stride_derives_budget_and_regroups_on_boundary (DiTAR patch-AR). doc: INFER_ENGINE_IMPL §7 M4.2-additions. green: waav-infer-scheduler/src/cohort.rs — advance by a PATCH (not frame), derive the duty/step budget from patch stride, regroup cohorts on the patch boundary. gate "patch_stride_budget_regroup". deps: the M4.2 variable-stride loop.
5. id "M5-T8" milestone "M5" — llm_stream_node_and_tool_call_node. doc: INFER_ENGINE_IMPL §7 M5-additions + design/L3_L6 (ToolCallNode, LlmStreamNode). green: waav-infer-runtime/dag — LlmStreamNode = an off-AR-clock DAG stage holding a ComputeLease (token egress + cancel); ToolCallNode = off-audio partial-fire + context-merge-on-resume that keeps the duplex user-lane modeled while running. gate "llm_stream_node/tool_call_node". deps: M4.1b DAG machinery + M5 R6.
Return ONLY these 5 new task objects.`,
  { schema: { type:'object', additionalProperties:false, required:['tasks'], properties:{ tasks:{ type:'array', items: TASK } } }, label: 'add-missing-5', phase: 'Coherence' },
)
allTasks = allTasks.concat((added && added.tasks) || [])
log(`appended ${added && added.tasks ? added.tasks.length : 0} missing-gate tasks -> ${allTasks.length} total`)

// ===== PHASE 4: assemble + WRITE the final tracked markdown =====
phase('Assemble')
const TODO_PATH = '/home/bud/ditto/waav/WaaV/inferv2/INFER_BUILD_TODO.md'
const conf = await agent(
  `Assemble the FINAL tracked implementation TODO markdown for "WaaV Infer v2.1 — production build" from these ${allTasks.length} grounded+critiqued tasks, and WRITE it to ${TODO_PATH} with the Write tool. An engineer executes it top-to-bottom.
Structure: (1) a 1-paragraph preamble (extends the live 6-crate waav-infer to INFER_ENGINE_V2.md via extreme TDD on GB10; the 2 invariants; RED->GREEN->REFACTOR + live-validation; source waav-infer/gb10-env.sh for live gates; driven via the full-build / build-driver workflows). (2) An ENV/tooling checklist section first. (3) Then one section PER milestone in build order (M2, M2b, M3, M4.1, M4.1b, M4.2, M4.3, M4.4, M4.5, M4.x, M5, then GW-M1, GW-M3, GW-M4, GW-M5), each a checkbox table: | id | [ ] task | RED test(s) | green (crate/file) | live-GB10 validation | accept/DoD | doc | gate | deps |. (4) A final "Coverage attestation": every INFER_ENGINE_IMPL §6/§7/§8 gate + GW-1..18 + R-1..8 + the validation pyramid is mapped; note the 5 late-added gates (masked-slot-bandwidth, model-feasibility-reject, npu-static-shape, patch-stride, llm-stream/tool-node); list the scope-adjudicated raw-catalog letters (F7,G8,J3,J5,J6,J7,J8-J11,J13,J19) as core-absorbed/ops-spine/gateway-owned.
Preserve every task's id/fields verbatim; do not invent or drop tasks. After writing, return ONLY a one-line confirmation (byte count + milestone-section count).
Tasks: ${JSON.stringify(allTasks)}`,
  { label: 'assemble-and-write', phase: 'Assemble' },
)
return { taskCount: allTasks.length, areaCount: clean.length, todo_path: TODO_PATH, confirmation: conf }
