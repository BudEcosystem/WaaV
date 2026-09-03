# WaaV Infer v2.0 — Extreme-TDD Implementation Plan

**Status:** plan · **Date:** 2026-06-17 · Extends the live 6-crate WaaV Infer (`/home/bud/ditto/waav/waav-infer/`) to the `INFER_ENGINE_V2.md` architecture. Every failure case in `/tmp/waav_failure_catalog.md` appears here as a **named failing-test-first gate**.

> **Method = extreme TDD in the codebase's OWN idiom.** The current code already practices it: `#[cfg(test)] mod tests` with a `NopLoader` test-double `GraphLoader`, `tmp_with_config(name, json)` fixtures, and assertions that typed errors (not panics) are returned (`model.rs:346-413`). Every step below is **RED (write the failing test) → GREEN (minimal impl) → REFACTOR**. No production code is written before its test. Each test name maps to a catalog gate (cited `[F#/G#/H#/L#/J#]`). Determinism rule (catalog H, #24067): bitwise cross-run determinism is impossible (atomic reductions) — tests assert **per-stream determinism + invariants**, never cross-device bitwise equality, except for the bit-exact native-parity STT/TTS gates that already pass on CPU.

---

## 0. Current seam (what we extend, verified by reading the code)

- `crates/waav-infer-core/src/model.rs`: `trait TtsModel{synthesize(text,voice,speed)->Vec<(ChunkMeta,Vec<i16>)>}`, `trait SttModel{transcribe(&[f32])->String}` (both COARSE/whole-utterance), `trait GraphLoader`, `enum LoadedModel`, `fn load_model(dir,loader)->(LoadedModel,arch)` (16-arm registry), `struct Manifest{architecture,precision,weights}`.
- `crates/waav-infer-backend-api/src/lib.rs`: `trait StaticGraph{run(Vec<NamedTensor>)->Vec<NamedTensor>}` + pure-data `EpKind/EpRequest/ActiveEp`.
- `crates/waav-infer-server/src/{engine.rs,torch_sidecar.rs,lib.rs}`: one model/process, `Arc<Mutex<Box<dyn TtsModel>>>`, `admission=Arc<Semaphore>`, `draining:AtomicBool`. No scheduler/batching/streaming/lifecycle.
- `torch_runtime/base.py`: `SttRunner.transcribe`, `TtsRunner.synthesize->Iterator[np.ndarray]` (already streaming-capable; the Rust side collapses it). Framing: `u32 total|u32 json_len|json|f32le PCM`.

**Invariant preserved across the whole plan:** the 16 registry arms + `StaticGraph` + Path-A one-shot models are **untouched**. They keep the coarse trait and ride a micro-batch stage. Only AR models gain the stepped seam. New code lands in two new crates (`waav-infer-scheduler`, `waav-infer-runtime`) + additive trait methods, so `git revert` of any milestone leaves a working engine.

---

## 1. New crates & seams (the shape the tests drive out)

```
waav-infer-scheduler/   # the lockstep + step-bucket engine, no backend deps (pure logic, fully unit-testable)
  src/slot.rs           # SlotTable<S>, exec-mask, masked≠absent discipline
  src/ring_kv.rs        # RingKvCache: (B,H,ctx,D), logical-position wraparound mask
  src/cohort.rs         # CohortKey(model, stride-class), variable-stride loop
  src/admission.rs      # schedulability ΣU≤bound, graded degradation, duty ledger
  src/marker.rs         # future-step marker heap, flush, per-stream lifecycle FSM
waav-infer-runtime/     # the per-tick driver wiring scheduler→model→egress
  src/arstep.rs         # trait ArStepModel (the stepped seam)
  src/prefix_cache.rs   # radix/fingerprint prefix cache (hybrid KV tier 1)
  src/numerics.rs       # NaN-detect→reject-frame, fp32 sampler guards
  src/egress.rs         # delta-streaming, explicit FINAL, drop-oldest ring
  src/watchdog.rs       # frame-progress, leak-reconciler, gpu-health hooks
```

**The stepped seam (additive — does NOT touch the coarse traits):**
```rust
// waav-infer-runtime/src/arstep.rs
pub struct StepInput<'a> { pub slot: SlotId, pub frame_idx: u64, pub conditioning: Option<&'a Conditioning> }
pub struct StepOutput { pub frame: Frame, pub eos: bool }   // Frame = codec tokens or PCM delta
pub trait ArStepModel: Send {
    fn prefill(&mut self, slot: SlotId, cond: &Conditioning) -> Result<PrefixKey, InferError>;
    /// Advance ALL active slots ONE stride. The driver passes the exec-mask; the model
    /// MUST treat masked rows as no-ops (state frozen) and accept substituted init tokens.
    fn step(&mut self, batch: &SlotBatch) -> Result<Vec<StepOutput>, InferError>;
    fn reset_slot(&mut self, slot: SlotId);              // transactional fan-out [F3]
    fn kv_footprint_per_slot(&self) -> KvFootprint;       // for the watermark [H3]
    fn stride_class(&self) -> StrideClass;                // for cohorting [R2/L6]
}
// LoadedModel gains: fn as_stepped(&mut self) -> Option<&mut dyn ArStepModel> { None }
// AR arms (voxtral/chatterbox/+Path-B) override it; one-shot arms (kokoro/whisper) return None → micro-batch stage.
```

---

## 2. Milestone M2 — the stepped seam + single-cohort lockstep (the unlock)

### M2.1 — `ArStepModel` seam + `as_stepped()` dispatch
**RED.** `tests::stepped_models_expose_seam_one_shot_models_dont`: build a fake AR model + a fake one-shot model (test-doubles, like `NopLoader`); assert `as_stepped().is_some()` for AR, `None` for one-shot; assert a one-shot ridden by the micro-batch path still returns its `synthesize` Vec unchanged (Path-A untouched).
**GREEN.** Add the trait + default `as_stepped()->None`; override on one AR test-double.
**Gate:** registry/Path-A invariant.

### M2.2 — `SlotTable` + exec-mask: **masked ≠ absent** (the central correctness law) `[F1,F2]`
**RED (these are the highest-value tests in the whole plan):**
- `masked_row_gets_substituted_init_token`: a `SlotBatch` with slots {0 active, 1 idle}; assert the driver overwrites slot-1's model input with the `initial`/BOS sentinel **before** `step()` (a fake model records its inputs); without it, the fake model's "embedding lookup" on the idle row would index a sentinel `-2`. `[F1]`
- `masked_mutation_is_frozen`: advance a batch where slot-1 is masked for 10 steps then active; assert slot-1's `(offset, ring_write_index, conv_state, sampler_rng_offset, partial_buffer)` are **byte-identical** before/after the masked steps — enforced by a single `where(exec_mask,new,old)` gate. `[F2]`
- `idle_then_resume_transcript_identical`: a slot that goes idle for N steps then resumes produces the **same** output as a never-idled slot (the multi-tenant bug that never shows in single-stream tests). `[F2]`
- `all_idle_short_sleeps_no_kernel`: empty exec-mask → the driver does NOT call `step()` and sleeps a bounded interval (assert via a fake clock). `[F6]`
**GREEN.** `SlotTable<S>{ slots: Vec<Option<Slot<S>>>, exec_mask: BitVec }`; the driver's pre-step substitution + a `MaskedCell` newtype whose only mutator is `set_where(mask, new)`. Make every per-slot field a `MaskedCell` so an ungated mutation **doesn't compile**.
**REFACTOR.** Enforce at the type level: `Slot` fields are private `MaskedCell`s; the only way to advance is `advance_where`.
**Gate:** `[F1,F2,F6]` — the "masked≠absent" discipline.

### M2.3 — `RingKvCache` wraparound `[F4]`
**RED.** Port Kyutai's published test vectors (`kv_cache.rs:260-327`) verbatim as `ring_kv_wraparound_vectors`: pre-wrap, exact-fill, post-wrap, mixed-mask; assert the per-cell logical-position mask = `[-inf,0,0,0,-inf]` on the wrapped row. `ring_kv_T_ge_context_guard`: assert `T<context` (or the abs-path) — no out-of-bounds. `reset_slot_recycle_byte_identical` `[F3]`: a fresh stream in a recycled slot == a never-used slot; channel-id guard drops a stale marker/output `[F3]`.
**GREEN.** `RingKvCache{ k,v:(B,H,ctx,D), positions:[u32;B], indices:[u32;B] }` + `indices_and_mask(seq_len,batch_mask)` reconstructing logical position; `reset_batch_idx` fans out (positions/indices=0, relies on mask not byte-wipe).
**Gate:** `[F3,F4]`.

### M2.4 — single-cohort lockstep driver + wall-clock pacing `[F10]`
**RED.** `lockstep_advances_all_active_one_stride_per_tick`; `step_wall_time_metered` (a `MODEL_STEP_DURATION`-equiv histogram, fake clock); `overrun_buffers_not_drops` (explicit overrun policy, assert per-stream buffer-depth gauge rises). Bench-anchored: assert flat-to-cohort-size throughput against a fake model with the measured §1.1 profile.
**GREEN.** The per-tick loop: gather inputs → substitute masked → `step()` → scatter outputs → meter. Path-B sidecar gains the multi-session `step` verb over the existing framing (slot-id + per-slot input → per-slot frame).
**Accept (M2):** GB10 serves ≥16 concurrent codec-AR streams at RTF<1; single-stream edge path unchanged; all `[F*]` gates green.

---

## 3. Milestone M3 — streaming egress, numerics survival, hybrid prefix-cache

### M3.1 — delta-streaming egress + explicit FINAL `[I1,G2]`
**RED.** `offline_concat_equals_stream_concat_byte_identical` (the #1 silent bug — `[I1]`); `stream_emits_delta_not_cumulative` (assert per-step payload is only the new samples, O(N) not O(N²)); `cancelled_stream_distinct_from_completed` (a FINAL frame on completion; a distinct CANCELLED terminal on barge-in — `[G2]`, must be distinguishable); `stream_closed_without_FINAL_is_failure`.
**GREEN.** Convert `TtsModel::synthesize`'s Vec into an incremental channel for AR models (thread the Python `Iterator` through the sidecar boundary — it already yields); a `TerminalFrame{Final|Cancelled|Error}` enum on the wire.
**Gate:** `[I1,G2]`.

### M3.2 — numerics guard: NaN → reject-frame (POLICY INVERSION) `[H1,H5]`
**RED.** `nan_logit_rejects_frame` (inject a NaN logit row → the frame is rejected: repeat-prev/codec-silence, NOT an argmax'd garbage token — `[H1]`); `fp16_softmax_uses_fp32_accum`; `tiny_temp_clamped` (`_MAX_TEMP=1e-2` clamp+warn — `[H5]`); `temp_zero_folds_greedy` (`_SAMPLING_EPS=1e-5`); `all_masked_logits_keeps_one_survivor` (`top_p_mask[-1]=false`); `nan_safe_pivot_uses_not_lt` (the `not(x<y)` idiom catches NaN).
**GREEN.** `numerics.rs`: an always-on `logits.is_nan().any()` reduction in the AR loop; the four sampler guards ported verbatim; **multinomial sampled OUTSIDE the captured CUDA-graph region** (resolves critique C2) or a graph-safe gumbel-argmax inside.
**Gate:** `[H1,H5]` + critique C2.

### M3.3 — hybrid KV: radix prefix-cache + anti-contamination `[L1,G1,R1]`
**RED.** `ref_audio_fingerprint_no_crosstalk` (two requests, same text, **different** ref-audio → different voice out, NOT contaminated — `[G1]`); `zero_shot_shares_prefix_subtree` (no conditioning → `extra_key=None`, prefix shared); `fingerprint_covers_all_codebooks` (ref-audios differing only in higher codebooks get different keys); `prefix_cache_hit_recovers_kv` (a returning voice hits the cache, suffix-ring starts fresh — assert the ~86% hittable path); `hash_is_sha256_not_xxhash` `[H]`; `per_tenant_salt_isolates` `[H]`.
**GREEN.** `prefix_cache.rs`: a paged content-addressed cache keyed `blake2b(full N-codebook conditioning)` for the deterministic prefix; the ring holds only the per-utterance suffix (two-tier KV, R1). Prefix-affinity routing hook for R5.
**Gate:** `[L1,G1,H]` — recovers the 86%, kills wrong-voice contamination.
**Accept (M3):** a 3-node CosyVoice2 DAG + a 2-node nested dots.tts DAG stream first-audio sub-300ms; offline==stream byte-identical; NaN→reject proven; prefix-cache hit proven.

---

## 4. Milestone M4 — stage-DAG, step-bucket, graded admission, production spine

### M4.1 — stage-DAG + decoupled per-stage batching `[G3,G6,G7,G11, RFC#2568]`
**RED.** `stages_have_independent_batch_sizes` (AR≥4, codec=1 — a uniform default causes audio gaps, RFC#2568); `pipeline_overlap_stageN1_A_while_stageN_B`; `back_pressure_parks_upstream_on_full_queue`; `idle_stage_blocks_not_spins` (the GIL-→Rust translation: block on `recv_timeout`/`Notify`, never `loop{try_recv}` — `[G3]`); `per_request_bookkeeping_capped` (10000→trim-5000 or a long-lived server leaks — `[G6]`); `fan_in_dynamic_wait_for_fn_no_deadlock` (conditional text-vs-audio branch — `[G11]`); `route_fn_must_return_in_static_topology` `[G11]`; `same_process_moves_ownership_not_serialize` (`Box<Payload>` across an in-process channel; fan-out clones the container, shares `Arc` only on immutable leaves — `[G5]`).
**GREEN.** `StageNode` schema (paradigm, batch_policy, substrate, inputs/outputs_to, `[stage.nested]`); typed bounded channels; the three micro-engine archetypes (AR-batch / micro-batch / streaming-window); dynamic `wait_for_fn` + topology-checked `route_fn`.
**Gate:** `[G3,G5,G6,G7,G11]`.

### M4.2 — step-bucket batcher + nested third execution class `[R2,L5]`
**RED.** `step_bucket_folds_cfg_x2`; `bucket_key_accepts_variable_N_incl_1` (IntMeanFlow NFE=1 → feedforward — `[L15]`); `nested_inner_batches_across_outer_lockstep` (the AR-outer step fans B hidden-states into the inner variable-NFE solve — assert the 38×@64 launch-bound profile on a fake); `co_eviction_drops_eos_from_all_inner_loops` `[R2]`; `streams_at_different_NFE_dont_share_inner_tick` `[L5]`.
**GREEN.** `cohort.rs` variable-stride loop; the nested composition (outer lockstep ∘ inner step-bucket per tick).
**Gate:** `[R2,L5,L15]`.

### M4.3 — schedulability admission + graded degradation `[R3,H3,H8,J20,L9]`
**RED.** `admit_iff_sum_utilization_le_bound` (ΣU≤bound, whole-stream-fit not first-chunk — `[H2]`); `non_zero_watermark_computed_exactly` (`Σ per-slot next-frame KV growth` — `[H3]`); `no_preempt_mid_utterance` (`[H2]`); `wall_clock_aging_promotes_no_dropped_call` (`[H8]`); `overload_ladder_sheds_LO_then_brownout_then_eDF_drop_then_reject` (graded, not pure-reject — `[R3,L9]`); `negative_slack_frame_dropped_with_PLC` (`[J20]`); `deadline_propagation_cancels_downstream` (`[J20]`); `bottleneck_stage_is_admission_gate_not_AR` (`[G6/L10]`); `prefill_firewall_budgets_on_predicted_latency_not_token_count` (`[L10]`).
**GREEN.** `admission.rs`: per-substrate duty ledger + shared-bandwidth ledger (unified-mem); the graded ladder; KV-length-aware firewall.
**Gate:** `[R3,H2,H3,H8,J20,L9,L10]`.

### M4.4 — the production spine `[J1-J23,H6,H7,F9]`
**RED.** `cooperative_cancel_checked_every_frame` (`[J15]`); `slot_freed_on_any_exit_RAII` + `leak_reconciler_alarms_on_slot_connection_mismatch` (`[J15,F9]`); `frame_progress_watchdog_fences_on_stall` (the only silent-hang defense — `[J16,H9]`); `pdeath_sig_set_on_sidecar` (`[H7]`); `vram_accountant_refuses_load_exceeding_projected_peak` (`[J2]`); `free_before_load_no_double_peak` (`[J2]`); `cuda_graph_eager_fallback_on_capture_failure_sm120` (GB10 hang scars — `[H4,J12]`); `cell_shard_fault_does_not_propagate` (one cell's fault isolates — `[J1]`); `poison_pill_quarantined_to_dead_letter` (`[J17]`); `dead_sidecar_fails_sessions_not_hangs` (3-tier sentinel + dead-flag fan-out — `[H6]`); `coordinated_omission_corrected_metric` (deadline-relative timing, dropped frames back-filled — `[J22]`); `metric_bucket_edge_at_0p08s` (`[J23]`).
**GREEN.** `watchdog.rs` (out-of-band threads); the VRAM accountant; cell/shard worker topology; the CO-corrected histogram; the input firewall + crash-counter; `prctl(PR_SET_PDEATHSIG)`.
**Gate:** the whole real-world spine `[J*,H6,H7]`.
**Accept (M4):** codec/encoder placement frees GPU bandwidth for ≥1.3× more AR streams on GB10; admission rejects/degrades rather than glitches at saturation; a killed sidecar fails sessions in ~1s; a cell fault loses only its cell.

---

## 5. Milestone M5 — variable-stride, dynamic-FR, MTP, full-duplex, transport

**RED (highlights):** `variable_stride_advances_per_step_not_fixed_frame` (`[R2]`); `dynamic_frame_rate_codec_cohorts_without_known_rate` (FlexiCodec — `[L6]`); `mtp_emits_multi_token_preserves_rectangular_lockstep` (`[L14]`); `sparse_kv_spec_decode_only_on_long_context_stt_path` (`[L13]`); `full_duplex_user_stream_always_modeled_barge_in_cancels_llm` (`[S2S]`); `pinned_attention_sink_prevents_wraparound_instability` + `paged_escape_for_long_form` (`[L12]`); `media_on_udp_control_on_tcp` + `tcp_nodelay_set` + `ws_send_ring_drop_oldest_bounded` (`[J21]`); `session_resume_replays_only_unacked` + `app_heartbeat_frees_slot_on_miss` (`[J18]`).
**GREEN.** The Moshi 9-item checklist (RQ-Transformer depth decoder, multistream+delay engine, full-duplex I/O, marker/flush, static-shape accel); MTP heads; the datagram media plane + control plane; LoRA voice/language adapters (`[J14]`); rainbow-deploy + drain-FSM + max-session-age (`[J4]`).
**Accept (M5):** Moshi full-duplex served lockstep-batched (exceeds upstream); one binary config-scales GB10↔B200; benchmarked against VoxServe / Nexus / the 560-streams/H100 Nemotron baseline `[L3,L4,L11]`.

---

## 6. The test-gate matrix (failure case → test → milestone)

Every entry in `/tmp/waav_failure_catalog.md` is a gate. Condensed (full mapping generated at impl time):

| Catalog | Test gate | Milestone |
|---|---|---|
| F1 masked init-token | `masked_row_gets_substituted_init_token` | M2.2 |
| F2 gate-every-mutation | `idle_then_resume_transcript_identical` | M2.2 |
| F3 slot recycle | `reset_slot_recycle_byte_identical` | M2.3 |
| F4 ring wraparound | `ring_kv_wraparound_vectors` | M2.3 |
| F5 marker/flush | `marker_fires_after_delay_plus_buffered` | M2.4 |
| F6/F10 idle+pacing | `all_idle_short_sleeps`, `step_wall_time_metered` | M2.2/2.4 |
| I1 delta-stream | `offline_concat_equals_stream_concat` | M3.1 |
| G2 FINAL/cancelled | `cancelled_stream_distinct_from_completed` | M3.1 |
| H1 NaN→reject | `nan_logit_rejects_frame` | M3.2 |
| H5 sampler guards | `tiny_temp_clamped`, `nan_safe_pivot` | M3.2 |
| L1/G1 prefix anti-contaminate | `ref_audio_fingerprint_no_crosstalk` | M3.3 |
| G3 no busy-spin | `idle_stage_blocks_not_spins` | M4.1 |
| G6 cap bookkeeping | `per_request_bookkeeping_capped` | M4.1 |
| G11 fan-in deadlock | `fan_in_dynamic_wait_for_fn_no_deadlock` | M4.1 |
| R2/L5 third class | `nested_inner_batches_across_outer_lockstep` | M4.2 |
| H2/H3 admission | `admit_iff_sum_utilization_le_bound`, `non_zero_watermark` | M4.3 |
| R3/L9 graded overload | `overload_ladder_sheds_LO_first` | M4.3 |
| J15 cancel/leak | `cooperative_cancel_every_frame`, `leak_reconciler_alarms` | M4.4 |
| J16/H9 silent hang | `frame_progress_watchdog_fences_on_stall` | M4.4 |
| J1 cell isolation | `cell_shard_fault_does_not_propagate` | M4.4 |
| J2 VRAM accountant | `vram_accountant_refuses_over_peak` | M4.4 |
| H4/J12 graph fallback | `cuda_graph_eager_fallback_sm120` | M4.4 |
| J22 coordinated omission | `coordinated_omission_corrected_metric` | M4.4 |
| L6 dynamic FR | `dynamic_frame_rate_codec_cohorts` | M5 |
| L14 MTP | `mtp_preserves_rectangular_lockstep` | M5 |
| J21 transport | `media_on_udp_control_on_tcp` | M5 |

**Discipline:** a milestone is "done" only when (1) every listed test was RED-first then GREEN, (2) clippy clean, (3) the existing 16-arm registry tests + all native-parity STT/TTS bit-exact gates still pass (no regression), and (4) the **validation pyramid** runs (catalog I4): offline-parity **+ streaming-playback + concurrent-load**, not offline alone. The edge inline path must stay zero-overhead (no scheduler/ledger/cell machinery) — a `single_stream_edge_pays_nothing` test asserts the inline mode constructs none of the M4 machinery.

---

## 7. v2.1 gap-closure — milestones + gates added by the 1,113-scenario coverage audit

The 10-family audit (651 SAT / 386 PARTIAL / 76 GAP) closed every shortfall additively. New gates below, grouped by the `INFER_ENGINE_V2.md §6` layer they implement. RED-first as always.

**M2 additions (correctness prereqs — promoted EARLY):**
- `acoustic_delay.rs` (F8, was M5): `acoustic_delay_ring_depth_max_delay_plus_2`, `pre_delay_codebooks_pad_forced`, `delay_write_read_alignment_vectors` (port Moshi `lm.rs` vectors). `StepOutput` gains per-codebook depth: `step_output_per_codebook_shape`.
- Duplex seam (L3): `duplex_user_stream_always_modeled`, `eot_head_emits_confidence`, `double_talk_policy_backchannel_vs_grab`, `multistream_slot_role_delay_sign`, `delay_sign_selects_task_mode`.
- `zero_d2h_sync_during_decode` (the 9 ms/step budget rests on it); `sidecar_state_slot_keyed_no_crosstalk` (C3). **LANDED (M2add-T9) — BOUND TO THE REAL DECODE LOOP (rejection fix):** the prior slice gated only the *control-flow phasing* (a seam-call journal proving `decode_audio` is hoisted out of the lockstep `step` window) and Python test-double fixtures — neither bound to the live per-step host crossings, so the stated "0 D2H/step" accept was asserted, not satisfied. This slice binds the gate to the ACTUAL chatterbox codec-AR decode loop. A `D2hLedger` (`crates/waav-infer-core/src/tts/chatterbox.rs`) is threaded through the SHARED `lm_forward`/`decode_body` primitives that BOTH the edge path and the lockstep stepped seam (`prefill_slot`/`step_slot`) drive, so it accounts the REAL per-stride device→host crossings — it can NOT be satisfied by a test-double that bypasses the real argmax/decode. It classifies each crossing: `Logits` (the bounded `[vocab]` greedy-argmax read at the real `argmax_with_penalty` site — SMALL + constant per stride, NOT O(audio); the **single remaining per-step host read on the Path-A ORT seam, tracked-open** pending on-device-argmax / IoBinding, the #1 engine item) vs `Waveform` (the O(audio) codec→PCM `wav.to_vec()` copy in `decode_body`). The accept the live loop actually satisfies: **0 in-loop O(audio) `Waveform` crossings per step** (the codec decode is hoisted OUT of the per-frame loop via a `LoopPhase::Decoding`/`Egress` phase gate) with **exactly one bounded `Logits` read per stride** (`logits_per_step()==1.0`, constant — it must never grow with the produced audio). The phase gate is a runtime tripwire: a `Waveform` crossing recorded while in `Decoding` returns a typed `ChatterboxError::HostSync` (the "won't even run" guard), proven RED-capable by `in_loop_codec_decode_trips_typed_host_sync_violation` (forcing the real `decode_body` mid-loop trips it). `decode_loop_has_no_cpu_numpy_per_step` drives the registry-loaded `ChatterboxTts` through the erased `&mut dyn ArStepModel` + the real lockstep `Driver` (the scheduler's exact path) at ≥4 concurrent and reads the ledger off the live loop. Tests (Rust): `chatterbox.rs::{zero_d2h_sync_during_decode, in_loop_codec_decode_trips_typed_host_sync_violation, decode_loop_has_no_cpu_numpy_per_step}`. The earlier Rust `serve.rs::serve_does_zero_d2h_sync_during_decode` phasing gate STANDS (it proves decode is hoisted out of the serve loop). Path-B `torch_runtime/base.py`: the `DecodeLoopGuard` runtime profiler (`d2h_per_step()==0` + typed `HostSyncViolation` tripwire) + static AST analyzer (`assert_no_host_sync_in_decode_loop`/`find_host_syncs_in_loops` flagging `.cpu()/.numpy()/.item()/.tolist()/.synchronize()` inside any `for`/`while`/comprehension loop body) are now BOUND to a REAL our-source loop via `GuardedArDecoder.decode` (the Python mirror of `step_slot`): `TestGuardBoundToRealDecodeLoop` drives a real on-device decoder (0 D2H/step) + a regressed per-frame-host-read decoder (trips the guard) and statically scans `GuardedArDecoder.decode`'s actual source — not just `_clean`/`_dirty_*` fixtures. Pre/post-loop syncs are not charged in-loop (masked ≠ absent); per-slot/per-decoder guards isolate the D2H count at ≥4 concurrent. Tests: `torch_runtime/test_zero_d2h.py`. **OPEN (tracked, NOT this slice):** eliminating the bounded per-step `Logits` host read itself (on-device argmax + ORT `IoBinding` device-resident outputs — the "IoBinding-on-StaticGraph-seam, 13%→2×" #1 engine fix) is the remaining work to reach a literal 0-host-crossings/step; the ledger already measures it (`logits_per_step()`), so that fix is a GREEN-tightening (assert `0.0`) when it lands.

**M2b — Feature edges (new milestone, L1):** `ingress_resample_anti_alias` + `no_chipmunk_on_wrong_sr`; `ssml_tags_map_or_passthrough_never_spoken`, `locale_normalization_per_synthesis_language`, `code_switch_script_segmentation`; `ctc_collapse`, `local_agreement_partial_stability`, `wordtiming_confidence_populated`, `aed_dtw_word_alignment_monotonic`; `transport_egress_downsample_anti_alias`, `repacketize_to_fixed_20ms_rtp`, `opus_inband_fec_on_loss`, `codec_and_resample_run_off_ar_clock`; `feature_stage_state_reset_per_slot` (every stage, not just AR); `bias_context_in_seam_resets_and_salts_prefix`. One `decode_repeat_ngram_guard` covers STT-hallucination + TTS-degeneracy.

**M3 additions:** `streaming_encoder_cache_delta_bounded` (the 3rd KV tier, L7). Precision (GPU-free, NopLoader idiom): `int8_file_never_lands_on_ort_cuda_ep`, `precision_resolves_per_active_ep`, `empty_kv_dtype_follows_weight_precision_q4f16` (`StaticGraph::input_types()`), `quant_variant_gated_by_per_substrate_accuracy_stamp` (incl. TTS-MOS).

**M4.1b — DAG machinery (new sub-milestone, L2):** `route_fn_returns_in_static_topology`, `wait_for_fn_conditional_branch_no_deadlock`, `multi_terminal_join_by_time`, `final_propagates_after_tail_drain_per_terminal`, `cancelled_distinct_from_final_through_dag`, `sentence_aggregator_commits_on_boundary`, `dag_slot_reset_fans_to_all_stages`, `dag_channel_id_drops_stale_occupant_output`, `cloud_stage_remote_cancel_and_failfast`, `barge_in_fans_out_to_all_stages_with_ack`.

**M4.2 additions (3rd-class, L4):** corrected math `nested_variable_nfe_T_step_sums_max_subbucket` (`T_step = T_ar + max_over_active(inner_i × T_inner)`), `sub_bucket_inner_by_nfe_within_one_outer_step`, `triple_nested_reset_slot_fans_out_to_inner_solver_state`, `patch_stride_derives_budget_and_regroups_on_boundary`.

**M4.3 additions — scheduler as a function (L4):** `scheduler_orders_by_risk_not_deadline_alone`, `binary_viability_yields_slack_when_safe`, `admit_iff_every_substrate_duty_le_S`, `bandwidth_duty_measured_via_dram_active_co_load`, `admit_iff_shared_bandwidth_duty_le_ceiling`, `roofline_class_serializes_two_bandwidth_bound`, `bottleneck_repicked_per_admit`, `masked_slot_bandwidth_charged_in_admission`, `reject_model_when_min_step_exceeds_frame_period`, `per_tier_reserved_duty_admits_gold_first`, `per_substrate_batch_knee_from_ridge_point`. **Restored v1.0:** `sustained_p99_breach_trips_drift_response_with_hysteresis`, `thermal_throttle_lowers_rated_max`.

**M4.4 additions:** `prefix_affinity_router_to_kv_holder` + `affinity_yields_to_duty_when_holder_saturated` (the `waav-infer-router`); `calibration_stamp_gates_readyz` + `admission_refuses_stale_stamp` + `calibration_stamp_cache_hit_skips_recalibration`; `teardown_aborts_collectives_before_destroy` + `restart_waits_on_vram_reclamation`; `mid_tick_inflight_recycle_deferred`; `boot_reserves_graph_pool_delta` + `warmup_gates_readiness`.

**M4.5 — Control plane + lifecycle + cross-cell (new milestone, L5):** `control_plane_drain_load_lifecycle_reject_api`, `reconnect_admission_rate_capped_per_replica`, `lifecycle_fsm_degraded_lowers_ceiling`, `lifecycle_fsm_draining_frees_on_refcount_zero`, **`two_concurrent_cross_cell_loads_serialize`** (the box-scoped singleton VRAM accountant — the real correctness gap), `second_stream_promotes_without_dropping_first` (config-tier auto-promote), `fault_migration_appends_kv_and_inner_latent_leased_buffer_masked`, `slot_cap_by_vram_capacity`, `multi_model_co_residency_admission`.

**M4.x — StagePlacer (new milestone, L7):** the 11-test block — `stage_placed_on_affinity_substrate`, `follow_immovable_weights`, `shared_host_buftype_zero_copy_on_uma`, `discrete_gpu_async_copy_double_buffered`, `relay_credit_backpressure_no_overflow`, `relay_notify_before_wait_no_deadlock`, `content_sniffer_terminates_on_cyclic_payload` (WaaV's prior scar), `shm_orphan_reaper`, `static_shape_bucket_padded_for_npu`, `ep_fault_degrades_op_to_forward_native_with_telemetry`, `hpu_wide_batch_not_static_b1` (+ `EpKind::Hpu`).

**M5 additions (L6):** R6 — `filler_fires_when_ttft_exceeds_budget`, `sentence_aggregation_streams_first_clause`, `barge_in_cancels_llm_reclaims_leftover`, `two_tier_fast_then_reasoning_parallel_fire`; `LlmStreamNode`/tool-node gates.

### §7.1 The completeness cross-table (META — closes the "named-not-gated" class by construction)

**Rule:** every mechanism named in `INFER_ENGINE_V2.md` (§0-§6) maps to ≥1 gate above; an un-gated mechanism is flagged `requires-gate-before-production` and may not ship. The audit's per-scenario verdict tables (`WaaV/inferv2/scenarios/coverage/*.md`) are the scenario→mechanism map; this §7 is the mechanism→gate map; together they form the scenario→gate closure recorded in `WaaV/inferv2/COVERAGE_ATTESTATION.md`.

**Post-closure invariant:** for all 1,113 scenarios, `mechanism ≠ ∅ ∧ gate ≠ ∅`. The 76 ex-GAPs each gained a mechanism (above + V2 §6); the 386 ex-PARTIALs each gained a named gate or concrete spec. Re-audit (a CI job re-running the 10 family auditors against the patched docs) is the standing regression for coverage itself.

### §7.2 Convergence pass — the v2.1 layers verified + deep-designed (`WaaV/inferv2/design/`)

The v2.1 closure was itself re-audited and deep-designed by 6 per-layer agents (each both convergence-verified its scenarios and wrote real Rust types + RED test bodies). Result: **composition with the lockstep core proven in every layer** (not asserted), residuals all cross-layer-dependency / orchestrator-owned / wiring; **~133 types + ~130 named gates (~108 full test bodies)** across the layer designs:
- `design/L1_feature_edges.md` — `waav-infer-features` crate, `StageState`/`FeatureStageNode`, anti-alias resampler (closes a real codebase TODO), BiasContext→prefix-key.
- `design/L2_dag_machinery.md` — `route_fn`/`wait_for_fn`/`Terminals`/`FinalGate`/`SpanAggregator`/`DagSlotReset`/`CloudStage`.
- `design/L3_L6_duplex_reasoning.md` — `DuplexStepModel`/`MultiStreamSlot`/`AcousticDelayRing`/`LatencyFiller`/`LlmStreamNode` (proves K=1 generalization preserves masked≠absent + one-graph).
- `design/L4_scheduler_function.md` — the computable objective + `DutyLedger` + `RiskSlack` + `Router` (proves the corrected max-NFE math with a worked 64-AR+8-CFM example).
- `design/L5_control_plane.md` — `ControlPlane` contract + `LifecycleFsm` + box-scoped `VramAccountant` (shows the cross-cell OOM race it serializes) + leased `Migration`.
- `design/L7_guards_kv_placement.md` — `StreamingEncoderCache` (3rd KV tier) + `StagePlacer` + precision resolver + `EpKind::Hpu`.

**Two NEW gates the convergence pass found (added here to keep §7.1 closed):**
- `final_gate_tail_predicate_is_per_archetype` (L2 Res-3) — `FINAL.after_tail_drain` must use a per-archetype "tail drained" predicate (vocoder crossfade-complete ≠ AR marker-heap-empty), else FINAL forwards a crossfade-tail early → truncated final chunk. → **M4.1b**.
- `nested_stage_reset_slot_clears_inner_latent_before_outer` (L2 Res-4) — `DagSlotReset` must clear the nested inner-solver latent **inner-before-outer, atomically** (else the G8 stale-batch landmine). Corroborated by L3 + the arch audit. → **M4.2**.

**One real codebase debt surfaced** (not a design gap): `crates/.../resample.rs` `EdgeResampler` has no anti-alias on downsample today (a documented TODO) — the L1 design adds an `AntiAliasResampler` (windowed-sinc + carried FIR history); the `no_chipmunk` / `transport_egress_downsample_anti_alias` gates fail until it lands.

---

## 8. Performance gates + the model-implementation contract (accuracy-preserving)

Folds `INFER_PERF.md` into the TDD plan. Every gate here is RED-first and carries the **accuracy gate** (V2 §7.3): a perf change is GREEN only when it's also bit-faithful (the AR-compounding identity test).

### 8.1 The model-implementation contract (what a batchable AR/duplex model MUST expose)
This is the canonical checklist a new AR/codec-LM/duplex model is reshaped into (additive — one-shot models like kokoro/whisper return `as_stepped()→None` and skip it). Per-arch work = decompose `generate()` into prefill+step, swap `cat`-append KV for ring-scatter, MaskedCell discipline, graph-safe sampling.
1. **R-1 Stepped seam** — `prefill/step/reset_slot/kv_footprint/stride_class` (M2.1).
2. **R-2 Fixed-shape batched forward** — one `[B,…]` forward, B/T=1/KV-shape constant; **K duplex-lanes + D codebooks fold as INNER dims, not batch axes** (one CUDA-graph covers it); idle slots masked-not-removed (M2.2).
3. **R-3 Per-slot ring-KV scatter** — `scatter_set` at `(offset+delay)%ctx` + logical-position mask (M2.3, F4 vectors).
4. **R-4 Exec-mask (masked≠absent)** — init-token substitution + `MaskedCell` gate on every mutation (M2.2, the won't-compile discipline).
5. **R-5 No in-loop host syncs** — gate `zero_d2h_sync_during_decode`; pre-allocated buffers, no per-step malloc (M2.4).
6. **R-6 Graph-safe sampling** — argmax/gumbel in-graph or multinomial out; NaN→reject-frame; fp32 sampler (M3.2).
7. **R-7 DuplexStepModel** (full-duplex) — K-stream `MultiStreamSlot`, per-codebook `StepOutput`, `TurnHead` (M2/M5, `design/L3_L6`).
8. **R-8 Variable-NFE inner head** (nested 3rd class) — per-stream inner micro-batch + co-eviction; `T_step = T_ar + max_over_active(inner_i × T_inner)` (M4.2, `design/L4`).
**Verdict (proven):** zero custom kernels required — fused-scatter-KV/masked-attn/depth-transformer = plain-ops or torch.compile-fuses-it; **fused RoPE+QKV = rejected on accuracy** (#2274); quantized-GEMM = vendored, off-path, excluded by the accuracy invariant.

### 8.2 Performance gates (new)
- **`run_bound` on the `StaticGraph` seam (THE #1 engine perf change)** — extend `waav-infer-backend-api` with an IoBinding/stateful-decode path: bind in/out `OrtValue` on the active EP, reuse a persistent on-device KV buffer written at `cache_position`. Gates: `kv_stays_on_device_across_steps`, `run_bound_output_bit_identical_to_run`, `no_h2d_d2h_per_decode_step`. Measured 13%→2× (grows with batch×ctx); captures ~8 Path-A AR models. → **M2**. **LANDED (M2-PERF-T1) — seam + WIRED + MEASURED:** `waav-infer-backend-api`'s **backend-agnostic** `IoBinding` (pure data, no ort type crosses the seam, `#![forbid(unsafe_code)]` per P-8) now **splits a stepped run's inputs into loop-invariant `constants` (uploaded once) and per-step `inputs` (varying)**, plus a `device_outputs` residency set and a constants `epoch`. `StaticGraph::run_bound` defaults to a host-materialized merge (`run(constants ++ inputs)`, `masked ≠ absent`) so CPU/NPU are correct out of the box. `waav-infer-backend-ort` **overrides** it with a **persistent `ort::IoBinding` held inside `OrtModel`, keyed on the epoch**: on the first call / an epoch change it `create_binding` + `bind_input`s the constants ONCE (their single, loop-amortized H2D copy — ort: bound inputs are "used in all future invocations until overridden"); every call re-binds only the varying inputs (overriding the prior step) and `bind_output_to_device`s the outputs (host-accessible `CUDA_PINNED`/`HIP_PINNED`), then `run_binding` → extract. `to_ort_value`/`extract_named_output` are shared with `run`, so `run_bound` is **bit-identical**. **The win is measured, not prose:** a deterministic `h2d_input_bytes` counter proves the 8-step estimator loop copies **86.2% fewer input bytes (7.22× reduction; eliminated == `(n_steps−1)·const`)**, and the **live GB10 CUDA EP** is **1.18–1.29× faster** over a 200-step Supertonic-scale loop. **The seam is wired into the production hot loop:** `waav-infer-core::tts::supertonic::flow_solve` drives `vector_estimator` through `run_bound` with the 5 CFM constants declared once per `synth_epoch` and only `noisy_latent`+`current_step` per step — `.run()` is no longer on that path. Gates landed: backend `run_bound_eliminates_constant_h2d_on_estimator_loop` (the byte/perf gate, asserts the eliminated bytes), `run_bound_rebinds_constants_on_epoch_change` (epoch-keyed cache hit within an utterance + rebind across utterances, no stale-constant bleed), `run_bound_estimator_loop_faster_on_cuda` (live wall-clock), `run_bound_matches_run_bit_identical` / `run_bound_keeps_io_on_device` (CPU + GB10 CUDA bit-identity), `run_bound_repeated_runs_bit_identical` (64-iter), `run_bound_four_concurrent_sessions_no_crosstalk` (≥4 sessions, invariant #2); core `flow_solve_drives_run_bound_with_constants` + `flow_solve_bit_identical_to_run_loop` + `flow_solve_typed_error_on_bad_output_len` (production-path wiring + bit-identity + typed error); api `run_bound_merges_constants_before_varying` + `run_bound_loop_reuses_constants_host_fallback` + the contract/typed-error mirrors. The standing **AR-compounding identity gate** stays GREEN. Follow-on still open: fully device-resident persistent-KV retention across steps (the `cache_position`-written buffer / O(N²)→O(N) for growing-cache AR decoders); `device_outputs` is the declarative hook in place. Deps: M2-PERF-T0 — satisfied.
- **SDPA backend selection** — `sdpa_backend_pinned_per_arch` (cuDNN/flash, the 40–135× lever), `sdpa_never_falls_to_math`, `flashinfer_excluded_from_sm12x`. → M2/M4 (HAL).
- **GQA-native** — `ring_kv_laid_out_at_native_kv_heads` (no MHA-expansion; 5.5–6.9×). → M2.3.
- **Batch-tiered kernels** — `cuda_graph_only_below_batch_knee` (graphs/compile help @B1, hurt @B32 — measured 0.73×). → M2.4.
- **Per-step kernel discipline** — `fp32_reduction_survives_fusion` (the #42325 weight-mul-native-dtype test), `compile_uses_epilogue_fusion_false`, `gemm_dims_padded_to_8` (zero-pad + −inf-mask), `cfm_scheduler_coeffs_cached`. → M3.
- **CPU tier** — `cpu_bf16_fp32_accumulate_only` (MLAS-SBGemm/AMX, never int8), `threads_pinned_one_per_physical_core`, `numa_bound`. → M4.x.
- **The universal accuracy gate (standing, blocks every perf change)** — `ar_compounding_emitted_codes_identical` (N-step loop, codes IDENTICAL not close) + `concurrent_output_bit_identical_to_serial`.

### 8.3 Per-onboarded-model perf fixes (each bit-identical)
- **Path-A AR (the `run_bound`/IoBinding fix captures all):** qwen3_asr, funasr_nano, whisper/moonshine/canary/cohere (enc-dec encoder-output + encoder-KV re-uploaded per token), voxtral, nemotron, supertonic (4 constant CFM tensors re-uploaded per step).
- **Path-B torch (HF `StaticCache` + `torch.compile(dynamic=False)` — bit-identical):** dia, dia2, csm, dots_tts, vibevoice, neutts_air, arkasr, granite, qwen3_tts. **LANDED (M2-PERF-T2):** all nine RUN the mechanism via `apply_decode_accel` (the §8.3 executable `PATH_B_DECODE_ACCEL` table, frozen `DecodeAccel`, typed `CompileDynamicError`/`KeyError`; a static source-consumption gate certifies every module actually CALLS the seam — dia/dia2's prior bare `generate()` no longer passes). The **StaticCache pin** (the HF `cache_implementation="static"` / a vendored runtime's own pre-allocated fixed-shape KVCache — dia2's `init_cache(max_steps=…)`) is **always retained** (a cache layout, verified bit-identical on the shared Qwen2 backbone). The **`torch.compile(dynamic=False)` half is GATED on bit-identity** by a `verify` probe — the accuracy invariant (#2274) is the higher law: **measured live on the real models, compile is bit-identical for GREEDY/argmax decoders (arkasr+granite ASR, neutts — RETAINED, the perf win) but NOT for SAMPLING bf16 AR-TTS decoders (dia 2012→559 tokens, dia2 729/768 codes flipped, csm/qwen3/vibevoice — AUTO-REVERTED to eager, bit-faithfulness wins; `AppliedAccel.compile_reverted` records the truth, never a false 'accelerated' claim)**. dots_tts: compile verified bit-identical + retained (its custom runtime grows its own dynamic inner-LLM KV, so the fixed-shape capture is defeated by recompile-churn — a follow-on perf refinement, accuracy-neutral). Vendored backbones resolved via typed `resolve_backbone` (NEVER a silent `getattr(model,"model",model)` whole-model fallback — qwen3's real Talker backbone is `talker.model`, not a nonexistent `.model`). Gates: `path_b_models_use_static_cache_and_compile` (table + every-module-calls-the-seam + vendored-resolve_backbone sweep), `path_b_compile_bit_identical_per_model` (LIVE on a real `Qwen2ForCausalLM` through real `apply_decode_accel`+StaticCache+`generate`, + real-model dia2/dots_tts load gates), the compile-gating revert/retain gates, and the standing Rust `ar_compounding_emitted_codes_identical` (green).
- **Path-B ORT estimators (IoBinding):** cosyvoice3 (keep `x` on-GPU across the CFM loop — the `.cpu().numpy()`-per-step fix), higgs_tts (KV host round-trip per frame, RTF 18.8).
- **omnivoice:** batch the CFG cond+uncond pair into one `[2,…]` forward (exact, CFG linear in the two logit sets).
- **CLEAN (no fix):** kokoro, melo, sensevoice, nemo_ctc.

### 8.4 Code patterns (canonical; see `INFER_GUIDELINES.md`)
The durable engineering rules — the stepped-seam shape, the `MaskedCell` discipline, the `run_bound`/IoBinding pattern, the SDPA-pin table, the fp32-fusion-safe-compile recipe, the AR-compounding identity-test harness — are factored into **`WaaV/inferv2/INFER_GUIDELINES.md`** as the standing code-pattern reference every model/backend PR follows.
