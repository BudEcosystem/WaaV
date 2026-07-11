# WaaV Infer — SOTA Implementation Plan (extreme-TDD, roofline-honest, all models × stages × quant × hardware)

**Date:** 2026-07-01 · **Branch:** `waav-infer-v2-build` @ `1130f97` · **Target:** GB10 sm_121 (measured roofline)
**Goal:** the optimal, adversarially-critiqued, low-level implementation plan that makes WaaV Infer beat vLLM-Omni, fixes the design-vs-wired ("wiring") problem, resolves conflicts, and universalizes across model architectures / pipeline stages / quantizations / hardware.

**How this was produced (provenance):**
- Custom reusable workflow `.claude/workflows/waav-sota-plan.js` (`/workflows` → `waav-sota-plan`): **10 deep-dive+design agents** (one per gap) → **2-hawk brutal critique panel each** (correctness/accuracy hawk + perf/portability/production/TDD hawk) → **conflict-resolving synthesis**. Run `wf_60a9204a-319`: **31 agents, 2.3M tokens, 466 tool-uses, ~26 min**. Raw output: `scratchpad/sota_plan_raw.json` (+ 352 KB of gap dossiers).
- **6 measured GB10 micro-benchmarks** (`scratchpad/microbench.py`, torch 2.12+cu130) grounding every perf lever.
- Grounded in the on-disk measured captures `perf/path-b/{dia2,cosyvoice3,csm,qwen3,voxtral,s2_pro,neutts,hibiki}/…` (ncu SpeedOfLight + nsys CPU/GPU attribution) and the accuracy-gated toolkit `profiling/perf_equations.py` (34-test gate) against `profiling/gb10_calibration.json`.
- Prior context: `WaaV/VLLM_OMNI_VS_WAAV.md` (the architecture comparison this plan operationalizes).

---

## 1. Thesis (the honest ceiling + the four winning axes)

**WaaV Infer will NOT beat vLLM-Omni on raw single-stream AR throughput — and pretending otherwise is the trap.** B=1 AR decode is memory/launch-bound *by GB10 physics*: arithmetic intensity ~1–2 ≪ the measured **ridge 433.5 FLOP/byte**, and there is no `torch.compile`/FlashInfer/SDP-backend-selector on sm_121 and no ORT IoBinding same-run alias. Measured: **a B=1 decode GEMM achieves 0.91 TFLOP/s = 1.06 % of the 86 TFLOP/s peak** (bench 3). dia2 stays RTF **1.86** even at its measured best.

**WaaV beats vLLM-Omni on the four axes where vLLM is structurally weak:**
1. **REALIZED LATENCY** — a chunk-knob TTFP taxonomy WaaV lacks today (TTFA == whole-utterance, 3–10 s → sub-second; cosyvoice3 3.34 s → ~600 ms).
2. **MEMORY** — in-process device-resident heterogeneous staging (cosyvoice3 already proves ~191 MB activation in **one** arena vs vLLM's **22 GB** process-per-stage bug #2318 → **≥4–7×**).
3. **ACCURACY** — byte-identical-by-default, a guarantee vLLM-Omni does not make (and its blog demonstrably oversells).
4. **PORTABILITY** — ONE declarative (arch × hardware × quant) contract with machine-checked accuracy stamps.

**The physics is measured, not asserted** (see §6): the three orthogonal byte-identical levers each recover launch/sync-bound idle — kill syncs (**96 µs/sync**, up to 740 ms/utterance), graph launches (**3.2×**), batch (up to **21.9×** GEMM ceiling, Amdahl-capped in practice); quant is a 4th, bandwidth lever (**2–4×**, because B=1 runs *at* bandwidth peak). No single lever wins alone — the plan **composes** them on the same serve path.

---

## 2. Two governing corrections (the critics overturned two premises — including one of mine)

### 2A. The Accuracy-Class Law (kills the "causal codec = free streaming" premise)
There is **exactly ONE** byte-identical-by-*construction* streaming primitive: **causal-CONV left-context** (proven only for vibevoice's conv VAE). Everything with attention / transformer / non-causal-stack / GEMM-stack / cross-kernel / cross-runtime is a *reassociation change of the same class as cfg_batch's refused batch-2 GEMM* and MUST ride an explicit WER/MOS-stamped path — never a "by construction" claim.

> **This corrects `WaaV/VLLM_OMNI_VS_WAAV.md`**, which wrongly proposed "engage `decode_committed_prefix` on causal codecs (SNAC/Mimi) — byte-identical." **Mimi is empirically non-causal** — verified at `arstep.rs:608`: `decode(body[..p]) ≠ decode(body)[..p]` at sample 0 in *both* bf16 and f32 (the length-dependent `_get_extra_padding_for_conv1d` right-pad + float non-associativity). Only the **SEANet causal conv tail** qualifies for TIER-0; the Mimi/S3Gen/DAC/CFM decoders are TIER-2 stamped.

**3-tier accuracy taxonomy** (enforced fleet-wide by the W0.1 ledger):
- **TIER-0 byte-identical (max\|Δ\|=0):** same-runtime/same-kernel invariance ONLY — CUDA-graph-vs-eager, on-device-sampler-vs-host-same-kernel, per-slot-ring-vs-solo, layout/threading. Default + verification mode.
- **TIER-1 ULP-bounded:** alternate-kernel same-precision swaps (XNNPACK vs MLAS, ORT-step vs tch-eager). A **new** explicitly-gated tier, never silent.
- **TIER-2 WER/MOS:** reduced-precision (fp16/int8/q4f16), lossy streaming left-context, GEMM-stack flow-batch, TRT, any cross-runtime assembled AR loop.

### 2B. The meta-fix: a Serve-Path Golden Ledger kills the design-vs-wired trap
The recurring root cause of WaaV's gaps is structural: **the harness enforces ACCURACY symmetrically but WIRING nowhere**, so world-class machinery (flow batcher, stage-DAG, advanced admission, streaming seam, `CfgBatchPlan`) went dormant with zero live callers. The **spine** (W0.1) is a serve-path golden ledger CI gate that **fails whenever any lever lacks a live caller + an absolute-golden diff** — closing the trap fleet-wide. Every downstream item lands behind it.

---

## 3. Verified findings (code-checked; actionable now)

| # | Finding | Evidence | Impact | Fix |
|---|---------|----------|--------|-----|
| **F1** | **Live admission ceilings use the 273 GB/s *datasheet* bandwidth, not the measured 198.5 GB/s** | `codec_ar_admission.rs:314,315,672,683,689`, `codec_ar_batcher.rs:656`, `engine.rs:1291 const GB10_PEAK_DRAM_BYTES_PER_S=273e9`; `lib.rs:853` even calls 273 "the datasheet theoretical ceiling" | **1.375× shared-bus over-admission** → blown frame deadlines under co-location | **W0.2** (retire 273e9 → 198.5e9 from calibration; CI grep gate) |
| **F2** | **Mimi decoder is empirically non-causal** | `arstep.rs:608` doc + probe | "causal-codec free streaming" premise (incl. in the prior comparison report) is false | **W2.3** prefix-equality probe classifies it `NonCausalByProperty`; **W2.4** stamped-lossy |
| **F3** | dia2 is **CPU-bound**, not compute-bound | HEAD commit + `perf/path-b/dia2`: `gpu_busy 0.06 / cpu_busy 0.89`, 11.3 s cuda_api, ~12,300 ms GPU-idle launch gaps | confirms launch-bound thesis; the raw-AR-RTF ceiling is shared physics | **W1.1/W1.2** (sync-kill + backbone graph) — necessary-not-sufficient |

---

## 4. Invariant hierarchy + conflict-resolution lattice

**Not a scalar priority** (a strict "ByteIdentity supreme" total order would say "keep exact fp32 KV and OOM rather than shed" — wrong). It is a lattice with a resolution operator:

- **I0 SAFETY (inviolable, enforced by REFUSING work, never by degrading numerics):** no box crash (UnifiedPoolBudget on the 130.6 GB shared pool), no hang (deadline-shed), no cross-tenant contamination (ChannelId/MaskedCell/ring-KV + DagSlotReset on recycle).
- **I1 CORRECTNESS:** every ADMITTED request is byte-identical to its whole-body solo golden. Relaxable to a WER/MOS-stamped `AccuracyClass` ONLY via an explicit typed path chosen by POLICY, **never** forced by resource pressure, **never** silent.
- **I2 STRUCTURAL (the mechanisms that guarantee I1, non-relaxable):** lockstep 1-stride/tick; per-slot ring-KV (GQA-native, not paged); cfg_batch batch-2 reduction refusal.

**Resolution operator:** when I1 (exactness) and I0 (fit/deadline) conflict → **I0-SHED** (reduce B, reject over-budget, evict lowest-criticality) — which preserves *both* (admitted requests stay byte-identical AND the box survives). Lossy is a POLICY choice via an explicit stamped path, never a pressure response.

**The 7 cross-gap conflict cells** (each machine-checked by W0.1 + the W2.3/W3.1 registries):
1. **streaming-prefix × ByteIdentity:** compatible ONLY for a codec passing the per-codec prefix-equality probe. SEANet conv-tail PASSES (TIER-0, W2.2); Mimi/S3Gen/DAC/CFM FAIL → TIER-2 (W2.4). REST byte-identical transport = **pcm only** (wav RIFF size known only at end).
2. **cross-request flow-batch × ByteIdentity:** FORBIDDEN on batch-stacked GEMM (batch-2 CFG reassociates cuBLAS, flips codes 3.9e-4 TF32). Byte-identical floor = non-reducing conv-vocoder batch (W1.4, TIER-0). Real GEMM-stack win = TIER-2 stamped, Throughput-only, EXACT-T bucketing (W4.5). Stream-concurrent per-slot (SB-CONCURRENCY) **DROPPED** (can't beat the shared 198.5 GB/s ceiling; tch has no CUDA-stream API).
3. **backbone-CUDA-graph × lockstep+ragged-membership:** naive B-bucketed cross-request capture FORBIDDEN. Resolution = **per-active-slot B=1 graph, host-constant row base + device-scalar length** (W1.2, TIER-0), LRU+mempool-bounded. dots stays per-patch (sequence-bucketing forks the take). csm/dia backbones stay eager (B27/B36 SDPA-kernel-selection scars).
4. **quant × ByteIdentity:** RequiresStamp per (model,precision,device-class,substrate) (W4.3, TIER-2). Byte-identical default. int8-on-ORT-CUDA FORBIDDEN (no int8-GEMM). Fork-B in-place-KV+graph FORBIDDEN-until-ort-fork (rc.12 IoBinding same-run alias gap — recorded, not silently retried).
5. **in-process multistage × UnifiedPoolBudget:** RequiresStamp against an **accounted transient reservation = SUM (not MAX)** of simultaneously-resident transients, reserve-on-entry/release-on-completion (W0.4). Over-budget → I0-SHED. (F1 fix makes the paired bus-duty gate honest.)
6. **TRT-depformer × TRT-backbone:** FORBIDDEN-to-compose (measured trt-both **1.987 > trt-backbone 1.859** — worse). TRT is TIER-2 Throughput; byte-identical CUDA-graph is default.
7. **SDPA-fused-pin × codes-AR ByteIdentity:** FORBIDDEN on codes-AR (omnivoice/dots/csm/dia2 — a fused kernel reassociates and forks the take). Allowed only where the gate is already WER/MOS (STT encoder prefill; the already-non-sample-identical cosyvoice3 ONNX flow estimator). Attention is ~1% of AR decode — not the lever there.

---

## 5. The plan — 6 waves, 24 items (each item = TESTS-FIRST → ACCEPTANCE → WIN → DEPENDS → RISK)

> Sequencing rule: **Waves 0–2 deliver the bulk of user-felt value** (spine + byte-identical collapse + TTFP). Waves 4–5 are deferrable. Go/no-go gates between waves.

### Wave 0 — Foundation: enforcement spine, roofline-truth, measurement, OOM floor
*Prerequisites, not deliverables. Nothing downstream is claimable until the wiring trap is machine-closed, the bandwidth constant is corrected, the null metrics are captured as gates, and the OOM scar is an accounted reservation.*

**[W0.1] Serve-Path Golden Ledger — two-axis (accuracy + live-caller) CI gate over the REAL serve loop.**
- TESTS-FIRST: `every_enrolled_lever_has_a_live_caller` (iterate `PERF_GATED_LEVERS`, FAIL any lever whose `WiringWitness.live_ctor` is unreachable through `engine.rs` — RED: CfgBatch has none); `serve_conformance_covers_every_registered_arm`; `serve_output_matches_stored_golden` (per (arm,lever) integer-codes-hash + PCM-hash + terminal-type vs an **absolute batch-1/whole-body** golden captured **through `engine.rs`**, on a corpus incl. a **ring-KV-wrap length** and an **over-MAX_SEQ reject** — catches the batch-2/flip class that concurrent==serial cannot); `dormancy_census_is_exhaustive` (every built-but-unwired artifact → `Wired(test-tag) | TypedReason`, no third state).
- ACCEPTANCE: CI fails if any enrolled lever lacks a serve-path golden OR any registry arm lacks a golden. Fast-smoke tier default; full byte-identical tier nightly (721 s-class).
- WIN: 0 perf; converts N dormant artifacts into serve-realized wins and prevents recurrence of the trap. **The spine every downstream item lands behind.**
- DEPENDS: none · RISK: heavy (boots every model) → tier it. Golden MUST be absolute (not concurrent==serial, which passes when both share the same bug). `CfgBatchPlan::two_pass` is a plan validator, not causally load-bearing — the witness must be the golden integer-code run.

**[W0.2] Roofline-truth fix — retire the 273e9 datasheet bandwidth; build every live `Ceilings` from measured 198.5 GB/s.** (Finding F1)
- TESTS-FIRST: `ceilings_use_measured_triad_bandwidth` (every live `Ceilings.bandwidth == 198.5e9` from calibration — RED at the sites hardcoding 273e9); `bus_duty_gate_refuses_at_measured_usable` (a set summing to >0.8·198.5 refused — RED today: admits up to 0.8·273 = 1.37× over).
- ACCEPTANCE: no live 273e9 site remains (CI grep gate); bus-duty refuses at 158.8 GB/s usable, not 218.
- WIN: correctness — kills the silent 1.37× shared-bus over-admission that blows deadlines under co-location; prerequisite for W3.1.
- DEPENDS: none · RISK: low; must sweep every site or the gate stays dishonest at the missed one.

**[W0.3] Measurement backfill — populate `ttfa_ms` + missing baselines (csm/zonos2/s2_pro/omnivoice-attn/dots-attn) + SDP & prefix-equality probes as GO/NO-GO gates.**
- TESTS-FIRST: `record_has_ttfa_ms` (RED: null fleet-wide); `csm_zonos2_have_rtf_baseline`; `sdp_probe_reports_backend_on_sm121` (bind `at::_fused_sdp_choice` — decides whether FusedAuto already lands cuDNN/flash); `codec_prefix_equality_probe_exists` (per-codec `decode(body[..p])` vs `decode(body)[..p]`); `attention_fraction_captured_inside_graph` (omnivoice/dots).
- ACCEPTANCE: these are **gates that KILL downstream items**: omnivoice/dots attention <10% → drop W5.3 SDPA arm; no causal-Mimi sustains RT → W2.x Mimi is TTFP-only-with-underrun; prefix probe fails → `NonCausalByProperty`.
- WIN: 0 perf; converts projected claims into measured go/no-go, killing speculative work before it's built.
- DEPENDS: none · RISK: some captures heavy; cheap insurance against a false premise.

**[W0.4] Accounted transient-reservation OOM floor — SUM (not MAX) of simultaneously-resident stage transients, reserve-on-entry/release-on-completion.**
- TESTS-FIRST: `transient_budget_is_sum_of_simultaneous_not_max` (MAX < SUM re-opens the box-crash); `colocated_second_transient_refused_before_malloc`; `reservation_released_on_ticket_drop`; `oom_shed_fires_on_real_try_admit_path`.
- ACCEPTANCE: reserve on stage entry / release on completion; concurrent transients SUM; over-budget sheds a typed 429 **before malloc** on the real `CodecArAdmission.try_admit` path; snapshot floor kept as defense-in-depth.
- WIN: closes the 21.7 GiB S3Gen / batch-24 KV **box-crash scar** (crashed the box twice) as an accounted reservation on the 130.6 GB shared pool — the safety floor that makes ALL Wave-3 staging landable.
- DEPENDS: W0.2 · RISK: over-conservative all-simultaneous bound may under-admit serializable transients — safe-side until W3.1 refines.

### Wave 1 — Byte-identical launch/sync collapse (zero relaxation)
*B=1 AR decode is launch/sync-bound; the accuracy-safe levers are killing the 12 per-frame D2H syncs, recovering the forfeited batched backbone graph, unifying capture infra, and the byte-identical conv-vocoder batch floor. All max\|Δ\|=0, all gated by W0.1.*

**[W1.1] Shared `nn::device_step` on-device sampling/stop/gather — kill the 12 per-frame D2H syncs (byte-identical).** *(measured: 96 µs/sync, cosyvoice3 top-p 3.78×/80 ms — bench 1 & 4)*
- TESTS-FIRST: `argmax_on_device_batch_preserving`; **TWO** parameterized samplers `topp_renorm_over_topk_equals_host` AND `topp_mask_to_neg_inf_full_vocab_equals_host` (csm uses full-vocab-mask — different Philox consumption; each reproduces its model's exact draw incl. multinomial replacement flag); `cosyvoice_keep_count_double_cumsum_equals_host_nucleus` (double-precision cumsum, 8-byte scalar D2H replacing the 54 KB `Vec<f64>`); per model `codes_identical_solo_vs_device` AND `…_vs_batched_twin`; `rng_free_inside_any_concurrent_region`.
- ACCEPTANCE: byte-identical codes solo-vs-device AND solo-vs-batched-twin for csm/misotts/qwen3_tts/s2_pro/cosyvoice3; nsys host_fraction drops; RTF non-regression.
- WIN: removes K−1 syncs/frame (csm 30, misotts 31, qwen3 14, s2_pro 8) + cosy's 54 KB host nucleus → 1 bulk read/frame (cosy llm_ar ~10–25 %, 98–245 ms). **The largest single byte-identical lever + the prerequisite that unlocks W1.2 graph capture and smooth streaming.**
- DEPENDS: W0.1 · RISK: TWO samplers are mandatory (one primitive forks csm's draw). Fresh `[K]` buffer must not alias ring-KV/CFG rows (dia2 deep-copy scar). Necessary-not-sufficient: dia2 is already sync-free yet 1.86.

**[W1.2] Per-active-slot device-scalar backbone CUDA-graph — recover the forfeited batched backbone graph.** *(measured graph win 3.2× on launch-bound work — bench 2; backbone byte-id graph −13.6% forfeited today)*
- TESTS-FIRST: `per_slot_graph_matches_eager_ring` (extends dia2 608/608); `append_full_masked_graph_device_len_byte_identical`; `slot_graph_no_host_op_during_capture` (D2H-during-capture counter == 0); `lru_bounds_graph_mempools`; `four_tenant_no_crosstalk_after_recycle`.
- ACCEPTANCE: batched B=16 agg-RTF with backbone-graph-ON **≥5% faster** than the eager-ring baseline, byte-identical codes; per-slot graphs LRU-bounded within the W0.4 budget; host-constant row base + device-scalar length only. Un-drops `dia2.rs:1156 set_cuda_graph(false)`.
- WIN: recovers the measured −13.6% backbone graph on the LIVE batched path (backbone is 47% of per-step device time; win grows with B — the ×B launch storm collapses to 1 replay/slot).
- DEPENDS: W1.1, W0.4 · RISK: scope to dia2-class FullMasked ONLY (csm/dia stay eager — B27/B36 SDPA scars). Per-slot graph identity must survive ChannelId re-stamp on recycle.

**[W1.3] Universal GraphCache infra — bucketed-LRU (causal-KV-masked only) + scalar-placeholders; fix dots thrash WITHOUT sequence-bucketing; retire 4 hand-rolled glues.**
- TESTS-FIRST: `bucket_bisect_left_causal_kv_only` (buckets apply ONLY to causal-KV-masked stages, NEVER bidirectional-DiT SDPA); `lru_evicts_oldest_and_frees_mempool`; `dots_scalar_placeholder_no_recapture` (temp/sigma/cfg as device `[1]` tensors → no recapture on condition change; dots STAYS per-patch); `dots_static_inplace_pos_attn_g_cond` (else a retained graph reads freed externals — the multi-slot fork RCA); `scalar_placeholder_output_byte_identical`.
- ACCEPTANCE: dots condition-recapture thrash eliminated while staying byte-identical (×1.04 today); LRU mempool sum < budget; the 4 hand-rolled `cuda_graph_enabled` bools collapse to one shared GraphCache.
- WIN: kills dots thrash; unifies maintainability; unlocks scalar-tunable graphs. HONEST: sequence-bucketing a bidirectional DiT is FORBIDDEN (softmax-over-pad reassociation forks the take).
- DEPENDS: W1.2, W0.4 · RISK: RNG-buffer-refill stays deferred; must reconcile with dia2 content-keyed per-slot re-seed if graphing sampled draws.

**[W1.4] SB-VOCODER — byte-identical, deadline-aware, cuDNN-deterministic-pinned conv/codec-vocoder batch (the safe flow-tail floor).**
- TESTS-FIRST: `vocoder_batch_byte_identical` ([B=8] HiFT/DAC/BigVGAN vs 8 solo, max\|Δ\|=0); `vocoder_no_reduce_over_N_op` (no eval-BatchNorm/cross-batch stat); `cudnn_deterministic_pin_holds_or_falls_back` (per-(B,T_pad) **output** byte-equality at runtime — not algo-introspection tch can't read; cache pass/fail per bucket); `deadline_aware_never_delays_lone_ready_utterance`.
- ACCEPTANCE: byte-identical `[B]` conv batch on the default path; opportunistic (co-batch only slots already at the vocoder stage within a bounded window; never delay a lone ready utterance); falls back per-slot on algo drift or reduce-over-N.
- WIN: ~1.1–1.15× aggregate on the vocoder sub-stage, ZERO accuracy risk; ships even if W4.5 underdelivers. The one place cosyvoice3 gets a tch-side win.
- DEPENDS: W0.1, W1.1 · RISK: deterministic algo may be slower (keep the byte-identity floor over the perf gate); wide transposed-conv vocoders may fall back (safe, ~0 win).

### Wave 2 — Streaming / TTFP (the flagship realized-latency win)
*TTFA==whole-utterance is the largest user-felt gap. Transport substrate first → the ONE construction-safe primitive → machine-checked codec classification → the stamped-lossy CFM flagship, gate-FIRST.*

**[W2.1] REST pcm chunked-transfer egress + un-drop codec deltas at BOTH sites + single-stream host-sync-stall overlap.**
- TESTS-FIRST: `rest_pcm_first_byte_before_wall` (chunked TTFB ≪ wall; Σ chunks byte-identical to buffered pcm — pcm ONLY); `committed_prefix_pcm_reaches_wire_both_paths` (RED: `engine.rs:2332` AND `codec_ar_batcher.rs:338` both drop today — fix BOTH; `Frame::Codec` STAYS dropped, committed-prefix PCM flows); `overlap_byte_identical_vs_forced_sync` (pure scheduling on the SINGLE default stream); `slow_reader_triggers_hangup_no_unbounded_buffer`.
- ACCEPTANCE: REST pcm clients get TTFP (first byte ≪ wall, byte-identical to buffered pcm); overlap re-based on the measured host-D2H-stall on the single default stream (NOT GPU concurrency — tch has no 2nd stream).
- WIN: closes the OpenAI-REST 0%-streaming hole; the transport substrate that makes W2.2/W2.4 realized.
- DEPENDS: W1.1 · RISK: wav default can't stream byte-identically (RIFF size at end) → pcm-only. ~zero standalone value without W2.2/W2.4 — sequence with them.

**[W2.2] SEANet causal conv-tail chunked decode — the ONE byte-identical-by-CONSTRUCTION streaming primitive.**
- TESTS-FIRST: `seanet_conv_tail_chunk_concat_byte_identical` (StreamCache chunked conv == whole-conv, max\|Δ\|=0 — construction-safe for causal CONV only); `conv_streamcache_disjoint_from_ar_ring`; `decode_committed_prefix_override_wired` (the seam at `serve.rs:1051` actually drives it).
- ACCEPTANCE: the SEANet causal conv tail streams byte-identically by construction; wired into `decode_committed_prefix` ONLY for the conv-tail portion. The Mimi transformer decoder is explicitly out of scope → W2.4.
- WIN: the genuinely zero-relaxation streaming component; narrow but ships-now with no WER gate.
- DEPENDS: W2.1, W2.3 · RISK: narrow/incomplete alone (Mimi transformer decoder is non-causal, F2).

**[W2.3] Per-codec prefix-equality probe + streaming stamp registry — `Streaming(passing probe) | NonCausalByProperty | StampedLossy`.**
- TESTS-FIRST: `every_tts_arm_is_a_streaming_decision_not_omission` (each arm overrides the seam OR registers a typed `NonCausalByProperty` — RED for silent-default arms); `mimi_decoder_classified_noncausal` (the probe FAILS, per F2); `streaming_enabled_only_behind_passing_probe_or_stamp`.
- ACCEPTANCE: "no override" becomes a typed DECISION; the executable prefix-equality probe is the machine-check for Compatible-vs-StampedLossy; wired into W0.1 so a future "optimize to early-emit" cannot silently ship non-byte-identical audio.
- WIN: 0 perf; the conflict-resolution machinery for the streaming law (converts `arstep.rs:551` doctrine into a machine-checked invariant).
- DEPENDS: W0.1, W0.3 · RISK: none numeric; value is preventing silent regressions.

**[W2.4] WER/MOS-stamped left-context streaming for non-causal vocoders (cosyvoice3 CFM flagship) — publish the pass/refuse table BEFORE building the seam.**
- TESTS-FIRST: `cosy_streaming_paired_gate` (chunked-vs-whole-body PAIRED per-utterance: primary hard metric **SI-SDR-vs-whole-body** + UTMOS/DNSMOS paired-delta CI + speaker-embedding cosine — NOT absolute MOS≤0.05 under proxy noise; WER alone insufficient — a codec fork holds WER flat while degrading timbre); `holdback_monotonic_emitted_never_re_decoded`; `default_off_no_stamp_is_whole_body_byte_identical`; `load_adaptive_knobs_stamped_at_worst_case` (dynamic-IC/keep-up-holdback PINNED to discrete stamped configs); routes through the existing `decode_committed_prefix`.
- ACCEPTANCE: run the harness on cosyvoice3/chatterbox/dia2 and **PUBLISH the pass/refuse table BEFORE wiring the seam**; per-model refuse-to-whole-body is LOUD; `ttfa_ms` is the metric (cosy 3.34 s → <700 ms). dia2 SCOPED OUT of sustained streaming (0.45× frame-budget headroom underruns) — TTFP-only + keep-up guard.
- WIN: cosyvoice3 TTFP **3.34 s → ~600 ms (~5–6×)** — the flagship; vLLM-Omni chunk-knob taxonomy (initial_ic + left-context + right-holdback + dynamic-IC + ref-code-on-first-chunk) ported as an ALGORITHM.
- DEPENDS: W2.1, W2.3, W1.1 · RISK: **THE thesis bet** — cosy's global-attention CFM (bidirectional over whole mel) may FAIL the MOS gate at usable left-context. If it fails, the non-causal headline collapses to pcm-transport + causal-conv-tail (thin). Gate-first mitigates; hold a fallback narrative (lead with the memory/accuracy win).

### Wave 3 — In-process heterogeneous staging (the memory win) + live roofline admission
*cosyvoice3 already PROVES the in-process cascade (RTF 0.67, ~191 MB, one arena). Generalize behind a live roofline admission gate + the OOM floor.*

**[W3.1] DCGM-free analytic `bytes_touched` estimator + wire LayeredAdmit/AtomicDutyLedger/KvFirewall as the live resource/feasibility/KV gate.**
- TESTS-FIRST: `analytic_bytes_touched_no_dcgm` (weights + ring-KV + activation-shapes × precision, or baked from `ncu.csv`/`record.json`, validated vs `perf_equations.py` roofline — RED: today `from_calibrated` is fed by `DcgmExporterDramActive` which GB10 cannot feed); `layered_admit_refuses_bus_saturating_set` (Σ duty > 0.8·198.5 refused while CodecArAdmission admits); `kv_firewall_refuses_overlong_before_alloc`; `atomic_admit_no_toctou_overadmit`.
- ACCEPTANCE: `admit_layered` is the SOLE resource/feasibility/KV verdict; concurrency/tenant/priority stay in CodecArAdmission (never decide twice); decision <10 µs, no per-admit heap alloc; estimator analytic (no DCGM) + validated vs captures + staleness stamp.
- WIN: correctness/safety — the roofline admission gate a concurrency+KV-blind CodecArAdmission cannot provide; gives DutyLedger the live home the DCGM path never had on GB10.
- DEPENDS: W0.2, W0.4 · RISK: the analytic estimator is the load-bearing NEW artifact (presented across G5 as existing but is a DCGM path GB10 can't run) — mis-calibration mis-gates; validate against captures.

**[W3.2] StageExecutor — in-process device-resident heterogeneous cascade via a SINGLE default stream (generalize cosyvoice3).**
- TESTS-FIRST: `dag_handoff_byte_identical` (cosyvoice3 llm→flow→vocoder through StageExecutor == monolithic golden, via single-default-stream happens-before — NOT multi-stream/record_stream which tch LACKS); `two_stage_shares_one_arena` (STT→TTS: exactly ONE CUDA context; peak ≤ Σweights + max-stage-activation + one arena, NOT N×0.90); `ar_stage_batchwidth_gated_by_stamp` (solo-refuser stays B=1; ragged-safe model co-steps at its STAMPED width); `dag_slot_reset_wired_no_cross_tenant`.
- ACCEPTANCE: in-process cascade RTF ≤ cosyvoice3 0.67; STT→LLM→TTS RTF<1; ONE arena; DagSlotReset fanned on recycle; AR BatchWidth per-model stamp-gated; Flow CFG cond/uncond stays INTRA-slot; retire `server/cascade.rs run_cascade` (go-live proof).
- WIN: **≥4–7× MEMORY reduction vs vLLM-Omni process-per-stage** (22 GB → <10 GB; cosyvoice3 proves ~191 MB activation in one arena) — the memory advantage generalized.
- DEPENDS: W3.1, W0.4, W0.1 · RISK: single-default-stream = memory + CPU-launch-pipelining win, **NOT** GPU-overlap (tch has no record_stream; multi-stream without it hits the caching-allocator reclaim hazard → silent corruption). Generic executor may need per-cascade tuning to match cosy's 0.67.

**[W3.3] DriftDetector → shed_victim live overload path; route eviction through DagSlotReset (ChannelId-bump-first).**
- TESTS-FIRST: `spike_400_sheds_not_hangs` (400-spike → typed StallTimeout/429, never hang); `drift_sheds_lowest_criticality_victim` (survivors byte-identical); `evicted_slot_reset_before_recycle` (ChannelId-bump FIRST, no cross-tenant leak); `peak_resident_under_watermark_at_oom_knee` (sweep B16/B24 — the real batch-24 KV realloc regime that OOMs).
- ACCEPTANCE: p99-breach → is_shedding → evict lowest-Criticality/least-progress; eviction transacts DagSlotReset; OOM guard tested at the B16/24 knee.
- WIN: production resilience — graceful shed instead of hang/OOM; the I0-safety operator made live.
- DEPENDS: W3.1, W3.2 · RISK: shed_victim MUST run DagSlotReset or leak prior-occupant state; the ChannelId-bump-before-fan ordering is load-bearing.

### Wave 4 — Universal portability + declarative onboarding + opt-in throughput ceiling
*ONE declarative (arch × hardware × quant) contract with machine-checked stamps, and the ONLY real memory-bandwidth throughput win (GEMM-stacked flow batch) as an explicitly stamped, opt-in tier.*

**[W4.1] Consolidate the 3 Path-A dtype seams into ONE + universal state-dtype for genuinely-hardcoded-KV models + `ElemType::BF16`.** *(measured quant lever: fp32→bf16 = 2.0× at bandwidth peak — bench 5)*
- TESTS-FIRST: `fp32_graph_byte_identical_after_migration` (the no-op law); `state_zeros_follows_graph_f16`; `dummy_past_prefill_shape_preserved` (don't break enc-dec prefill); `bf16_export_not_silently_f32`.
- ACCEPTANCE: the 3 seams (`precision.rs empty_kv_dtype`, `encdec.rs input_dtype/dummy_past`, `cohere.rs empty_past`) collapse to ONE; the ~2–4 genuinely-hardcoded-KV models load q4f16/fp16 without crash; fp32 byte-identical; `ElemType::BF16` closes the silent-f32 hole. (Enc-dec STT family is ALREADY graph-driven — don't double-count the inflated 8→22.)
- WIN: quant-loadable across the genuinely-affected Path-A models; the fp16→q4f16 ~2.75× weight-footprint capacity win is DORMANT until a real non-voxtral q4f16 export is measured (don't book it realized).
- DEPENDS: W0.1 · RISK: preserve the `dummy_past` prefill contract (naive zeros break enc-dec). Capacity claim is enabling, Amdahl-diluted until W1.2.

**[W4.2] Declarative `ServingPolicy` (RING axis) + `ServeStamp` default_green — absorb the ~131 env knobs, parity-pinned.**
- TESTS-FIRST: `resolve_matches_legacy_table` (all 15 batched arms == today's EXACT tuple incl. qwen3's CPU-force quirk + pocket_tts/higgs false-default; EXHAUSTIVE — an un-migrated arm fails the build); `manifest_overlay_wins`; `env_override_wins`; `unstamped_arch_defaults_solo`; `serve_stamp_is_per_hardware`.
- ACCEPTANCE: the 15 `use_ring` blocks + 45 inline env-parses collapse to one resolver + a per-arch data row; **RING axis ONLY** (graph/TRT stay resolved in-load until a separate seam threads `&Manifest`); byte-identical (selects among pre-gated paths).
- WIN: ~350 duplicated lines + 45 env sites → one resolver; new known-arch checkpoint = **0 code**; new arch = 1 row + 1 line. 0 perf (selection only).
- DEPENDS: W0.1 · RISK: parity enumeration MUST be exhaustive or an un-migrated arm silently regresses to Solo. Refactor/onboarding win, not a perf unlock.

**[W4.3] Unified precision+quant manifest + per-(model,precision,substrate) WER/MOS stamp fleet harness + Path-B PrecisionMode stamp-gating.** *(measured: dia2 native-bf16 RTF 1.596 vs shipped 1.928 = −17%, WER 0.0129≈0.0337 equivalent)*
- TESTS-FIRST: `manifest_stamp_gates_admission`; `pathb_precisionmode_calls_stamp_gate` (RED: today PrecisionMode flips the native fork with NO stamp check — a REAL ungated hole); `quant_stamp_is_owned_string`; `tts_stamp_requires_wer_AND_mos_AND_speaker_sim` (byte-identity FORBIDDEN as the TTS quant gate); `unified_precision_type_across_backends`.
- ACCEPTANCE: unified `AdmittedPrecision` across Path-A/Path-B; Path-B native-fork now stamp-gated (closes a real hole); TTS gate = UTMOS/DNSMOS + speaker-cosine + WER (human MOS offline); dia2 native-bf16 becomes a manifest choice; fleet harness arch-by-arch.
- WIN: realizes the measured dia2 −17% as a declarative knob (bf16 is the measured BEST, not the shipped f32-sandwich); evidence-backed quant admission fleet-wide (today dia2-only).
- DEPENDS: W4.1, W0.1 · RISK: the fleet harness (two heterogeneous drivers) is undersold and is critical-path for W2.4/W4.5; MOS offline-stamped, not live-CI.

**[W4.4] EP-golden 3-tier accuracy gate + vendor-driven ORT auto-routing + XNNPACK on-box proof + device-class CapabilityMatrix.**
- TESTS-FIRST: `xnnpack_whisper_tiny_ulp_bounded_not_byte` (Tier-1 ULP — NO fp32 EP is bit-identical to MLAS); `auto_probe_is_vendor_driven` (synthetic Intel DeviceCaps → probe OpenVINO first — RED: static list drops it); `vendor_cpu_when_no_accelerator`; `router_refuses_unstamped_ep_bootstrapped_from_incumbent`; `capability_matrix_keyed_by_device_class`; `int8_reduced_precision_requires_wer_mos_stamp`.
- ACCEPTANCE: 3-tier taxonomy enforced fleet-wide; **XNNPACK-vs-CPU proven on-box** (the ONLY non-NVIDIA EP that executes on GB10; OpenVINO `.so` fails to load here → CI-runner-gated); auto-probe vendor-seeded but never overrides a present CUDA device; invented `est_speedup` constants replaced by measured; matrix consulted in the LIVE select (anti-shelfware).
- WIN: portability COVERAGE + honesty — 60+ ONNX archs × dylib-built EPs become accuracy-stamped; the router stops quoting fiction.
- DEPENDS: W4.3, W0.1 · RISK: byte-identical is UNACHIEVABLE cross-kernel (XNNPACK≠MLAS) — Tier-1/2 only; on-box proof is XNNPACK-only, every other EP CI-runner-gated with zero GB10 evidence.

**[W4.5] SB-STACK — WER/MOS-stamped true GEMM-stacked flow batch (PerfMode::Throughput, EXACT-T bucketing), the throughput ceiling.** *(measured batching ceiling: B8 4.8× / B16 9.1× — bench 3; memory-bound estimator → near-linear)*
- TESTS-FIRST: `flow_stack_refuses_without_stamp` (typed InferError unless a passing stamp — fail-closed, mirrors `cfg_batch.rs:182`); `flow_stack_never_default` (Accuracy never selects it); `flow_stack_exact_T_no_intra_bucket_pad` (EXACT T — no conv temporal contamination from zero-pad receptive-field bleed); `flow_stack_gate_paired`; `precomputed_noise_outside_stacked_region` (omnivoice/dots: draw all n_steps noises SERIALLY per row BEFORE the stack — zero RNG inside, else the shared global libtorch generator forks).
- ACCEPTANCE: 4–8× aggregate at B8–16 on the flow tail (memory-bound estimator → near-linear); stamp-gated, Throughput-only, fail-closed; EXACT-T bucketing; deterministic cuDNN/cuBLAS algos pinned so the stamp is reproducible; admission (W3.1) paces the per-tick bucket budget.
- WIN: the biggest raw throughput number (4–8× flow tail; omnivoice ~4–6×) — LOSSY, opt-in, never default. Where "beat vLLM-Omni on throughput" honestly lives.
- DEPENDS: W1.4, W4.3, W3.1 · RISK: SB-CONCURRENCY DROPPED (can't beat 198.5 GB/s; no tch CUDA-stream); omnivoice/dots RNG-in-loop MUST be precomputed serially; cosyvoice3's estimator is ORT (separate dynamic-batch path).

### Wave 5 — Scoped, lower-confidence follow-ons (gated behind a probe/baseline)
**[W5.1] zonos2 on-device MoE dispatch — byte-identical device-indexed SEQUENTIAL fold (eliminate ~23 host reads/frame).** Keep the EXACT sequential per-kk fold + per-expert GEMMs (NO bmm-reduce/scatter_add — reassociate/atomic-nondeterministic). Baseline captured FIRST (W0.3). WIN: worst per-frame sync count; 15–30% MoE-block, roofline-only until measured. RISK: lowest-confidence (no baseline).

**[W5.2] Portable fp16/q4f16 device-KV via byte-identical two-buffer ping-pong, wired to the BATCHED serve path.** *(measured device-KV crossover — bench 6: no win <B8, ~2.3× near B24)*. fp16 speedup re-MEASURED (< fp32's 2.34× since halving kv_bytes shrinks the eliminated restream). Fork-B in-place+graph Forbidden-until-ort-fork (rc.12 IoBinding alias gap). RISK: B=1 win ~0; batched-only.

**[W5.3] Fused attention where the gate is WER/MOS — cosyvoice3 ONNX flow estimator (contrib op) + long-context STT encoder prefill; SDPA-pin FORBIDDEN on codes-AR.** SPIKE-FIRST: prove via ncu that `softmax_warp_forward → 0` before committing (ORT may not fuse a DiT-layout estimator). Realistic win <5% of cosy wall (bounded by the measured 6.7% softmax). RISK: the 215→11 ms vLLM number is their large-context diffusion path, non-transferable.

**[W5.4] Path-B ONNX-step-export portability tagging — honest `PortabilityClass` surface with a WER/MOS end-to-end gate.** `PathAOnnxNative | OnnxStepExportable(Stamp) | CudaBound(reason)`. Honest outcome: mostly CudaBound tags, not new runnable models. RISK: lowest architectural leverage.

---

## 6. Measured GB10 micro-benchmarks (the physics grounding — `scratchpad/microbench.py`)

| Bench | Lever | Result |
|---|---|---|
| 1 | **Sync tax** (W1.1/W5.1) | `.item()` D2H = **96.4 µs/step (6.5×)** pipeline-flush → **740 ms/utterance** csm/misotts (32/frame), 231 ms s2_pro |
| 2 | **CUDA graph** (W1.2/W1.3) | 200 tiny ops: **1459 → 455 µs (3.2×)** via graph replay |
| 3 | **Batching** (W3.2/W4.5) | B=1 = **0.91 TFLOP/s (1% of peak)**; per-item **B4 2.7× · B8 4.8× · B16 9.1× · B24 21.9×** |
| 4 | **cosyvoice3 top-p** (W1.1) | on-device vs `Vec<f64>` D2H: **454 → 120 µs (3.78×)**, 80 ms/utterance |
| 5 | **Quant** (W4.1/W4.3) | B=1 runs *at* bandwidth peak; time ∝ weight bytes → fp32→bf16 **2.0×**; int8 ~4×, q4 ~8× |
| 6 | **device-KV** (W5.2) | host re-stream ∝B²: B24 = **240 µs/stride** → device-KV avoids it (measured 2.34×@B24) |

The decisive number is **bench 3's 0.91 TFLOP/s at B=1** — it *proves* the thesis: single-stream voice decode is nowhere near compute-bound, so batching + sync-kill + graph + quant are the levers, and the "55×→1.8×" walk-back is the Amdahl-capped realization of a real 9–22× GEMM ceiling.

---

## 7. Coverage attestation (model-arch × stage × quantization × hardware) + honest exclusions

**Model archetypes:** codec-AR (dia2/csm/qwen3_tts/misotts/s2_pro/voxtral_tts/indextts2/higgs/neutts/vibevoice) → W1.1/W1.2/W4.2; continuous-CFM (cosyvoice3) → W1.1/W1.4/W2.4/W4.5/W5.3; masked-diffusion (omnivoice) → W1.3/W1.4/W4.5; continuous-DiT patch-AR (dots) → W1.3 (scalar-placeholder only)/W1.4/W4.5; MoE-AR (zonos2) → W5.1; enc-dec STT (whisper/moonshine/canary/nemotron) → W4.1/W4.4; CTC/RNNT (sensevoice/nemo_ctc/parakeet) → W4.1/W4.4; decoder-LM STT (voxtral) → W1.1/W5.2; S2S duplex → W3.2 (carried, not a focus); utility (enhance/diarize) → W4.1/W4.4.
**Stages:** STT enc/dec (W4.1/W4.4/W5.3), backbone-AR (W1.1/W1.2/W1.3), depth/depformer (W1.1), flow/CFM/masked-diffusion (W1.4 TIER-0, W4.5 TIER-2, W2.4 TIER-2), vocoder/codec-decode (W1.4/W2.2), cross-model cascade DAG (W3.2).
**Quantization:** fp32 (TIER-0 default everywhere); bf16 (W4.3, dia2 −17% measured); fp16 (W4.1+W5.2); int8 (W4.1+W4.3; EXCLUDED on ORT-CUDA — no int8-GEMM); q4f16 (W4.1/W4.3/W5.2).
**Hardware:** Path-A ORT — CPU (TIER-0 ref), CUDA, XNNPACK on-box (W4.4, TIER-1, the ONLY non-NVIDIA EP that executes on GB10), OpenVINO/MIGraphX/CoreML/QNN (W4.4, CI-runner-gated). Path-B tch — CUDA primary.

**Honest exclusions (stated, not hidden):** no item crosses dia2 (or any launch-bound AR model) under RTF<1 (best 1.86; shared physics). Streaming byte-identical = ONLY causal SEANet conv-tail. Flow-batch byte-identical = ONLY non-reducing conv-vocoder. Multi-stream GPU overlap EXCLUDED (tch has no record_stream). Non-NVIDIA on-silicon proven ONLY for XNNPACK/CPU. TRT is TIER-2 Throughput only (doesn't compose with depformer). S2S/duplex realtime carried structurally, not advanced by this plan.

---

## 8. Plan red-team (the honest exposures — read before committing)
1. **It's a PROGRAM, not a sprint** (24 items, multi-quarter). If cut: Waves 0–2 deliver the bulk; 4–5 deferrable. Stage with go/no-go gates; don't present as one deliverable.
2. **The flagship (W2.4) is a thesis bet** — cosy's global-attention CFM may FAIL the MOS gate at usable left-context (receptive field effectively unbounded). Gate-first mitigates; pre-commit a fallback narrative (lead with memory + accuracy if TTFP under-delivers on the high-volume models).
3. **Nothing crosses dia2 under RTF<1** — the win is memory/TTFP/accuracy/portability, not raw single-stream AR RTF (shared physics ceiling).
4. **Measurement-dependency fragility** — W5.1/W5.3/W1.2-delta/W2.4-ttfa rest on captures that don't exist until W0.3. Treat W0.3 probes as hard go/no-go; if skipped under pressure, downstream ships on projections (the exact failure the critics caught).
5. **The spine (W0.1) is heavy and centralizing** — a single nondeterministic golden blocks the whole fleet's CI; the census front-loads reconciliation (the codebase has drifted since the dossiers — e.g. `dynamic_fr.rs` now references StepBuckets).
6. **tch/ort platform ceilings are hard walls** — no CUDA-stream/record_stream (W3.2 is memory-win only), no torch.compile, no SDP-selector, no ort IoBinding same-run alias (Fork-B 30× deferred). The largest theoretical wins (single-batched 30×, GPU-overlap) are explicitly OUT.
7. **Stamp-harness over-reliance** — W2.4/W4.3/W4.5 all depend on a WER/MOS/speaker-sim harness currently dia2-only; if its fidelity under-delivers, the "never silently lossy" guarantee leans on unproven measurement.
8. **The admission estimator (W3.1) is an unbuilt artifact** — presented as existing but is a DCGM path GB10 can't feed; least-de-risked piece of the safety story.
9. **S2S/duplex under-served** — the plan is TTS/STT-centric; if realtime full-duplex S2S is the strategic priority, this under-serves it.
10. **Integration risk between waves** — the critical path W0.1 → W1.1 → W1.2/W3.2 is three-deep before the first measured launch-bound win; any slip cascades.

**NET:** the plan is roofline-honest and invariant-safe, and correctly reallocates effort away from the killed designs (SB-CONCURRENCY, over-decode, growing-backbone graph, paradigm-derivation). Its biggest exposure is that the two *headline* wins (TTFP flagship, in-process staging RTF) are the two *least certain*, while the *certain* wins (memory, accuracy, portability honesty, the enforcement spine) are less viscerally "beat vLLM." Sequence and gate accordingly; do not commit the flagship latency claim publicly until W2.4's pass/refuse table is measured.

---

## 9. Re-running / iterating the analysis
- Workflow: `Workflow({ name: "waav-sota-plan" })` or `Workflow({ scriptPath: ".claude/workflows/waav-sota-plan.js" })`. Edit the `GAPS`/`MEASURED` constants to re-scope; resume a run with `resumeFromRunId`.
- Micro-benchmarks: `source /home/bud/torch212_trt_venv/bin/activate && python scratchpad/microbench.py`.
- Every perf claim here is either a captured measurement (`perf/path-b/…`, `scratchpad/microbench_results.md`) or a roofline projection via `profiling/perf_equations.py` — never a datasheet number.
