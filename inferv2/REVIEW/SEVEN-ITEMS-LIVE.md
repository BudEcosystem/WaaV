# Seven WaaV-Infer System Issues — LIVE Evidence Matrix

**Date:** 2026-06-24 · **Branch:** `waav-infer-v2-build` · **HEAD:** `ae98dd6` (no commit made)
**Box:** GB10 (Grace-Blackwell sm_121), 121 GB unified · `free -g` at start: 28 GB free / 85 GB buff-cache / 61 GB swap free
**Env:** `source gb10-env.sh` (ORT-CUDA 1.27 sbsa, libtorch via PyTorch 2.12+cu130) sourced before every run.
**Method:** for each item, found the gate(s) by grepping the named test fns / commits, then RAN them live with
`--nocapture` (live-GPU gates with `--ignored --test-threads=1`). Real result quoted below — not a claim.

---

## The 7-row matrix

| # | Item | Gate(s) run | Live result (quoted) | Commit | Verdict |
|---|------|-------------|----------------------|--------|---------|
| 1 | fp16/quant on CUDA (voxtral-q4f16 + cohere-fp16) | `voxtral_q4f16_nobias_runs_on_ort_cuda_byte_identical`; `cohere_fp16_nobias_runs_on_ort_cuda`; `voxtral_and_cohere_serve_on_ort_cuda_via_registry` | voxtral q4f16: ORT-CUDA transcript **BYTE-IDENTICAL** to ORT-CPU (decoder EP=cuda); cohere fp16: **EXACT char-identical: true / 100.0%** (EP=cuda); registry `load_model_at(dir,Cuda)` serves **both** identical to CPU. 3/3 pass | `59f1adc` | **PASS** |
| 2 | Full e2e fp16 input-dtype casts | `feed_float_f32_graph_is_bit_identical`, `feed_float_f16_graph_widens`, `feature_inputs_stay_f32_by_name`, `empty_kv_dtype_follows_weight_precision_q4f16` (4/4); + on-hardware `live_ragged_batched_forward_bit_identical_and_scales` | unit 4/4 pass; **on-hardware chatterbox CUDA: ragged bit-identity PASS** — 4 slots at distinct lengths [18,74,67,60], codes identical batched-vs-per-slot through the `feed_float`-built `inputs_embeds` (input CAST, not just output read); 1.43× batched speedup | `e2ff596` | **PASS** (see note) |
| 3 | Shelfware wire-or-downscope (S2S engine-served + the other named components) | `s2s_seam_registered_and_object_safe`; `lfm2_audio_s2s_via_registry_engine_served`; `engine_serves_inprocess_torch_hibiki_s2s_byte_identical_to_standalone` + a live-caller audit of 8 named components | S2S seam pass; lfm2 S2S engine-served **BYTE-IDENTICAL** to core round_trip; hibiki S2S engine-served reply **15360 samples == standalone** (byte-identical). 3/3 pass. Other-shelfware disposition below | `56254bf` | **PASS** |
| 4 | Scheduler hazards (DutyLedger TOCTOU + migration stale-epoch) | `atomic_admit_and_commit_is_one_critical_section`; `no_clone_admit_matches_clone_based_projection_bit_identical`; `concurrent_admit_and_commit_never_over_admits`; `monotonic_epoch_prevents_double_admit`; `dest_admit_isolates_...`; `leased_buffers_isolate_across_4_concurrent_migrations`; `cadence_migration_rejected`; `fault_migration_refused_on_key_mismatch` | **64 threads → exactly 8 admits** under S=0.8 @0.10 each (TOCTOU closed; never 9); migration `StaleEpochReject` prevents double-admit (lower epoch fenced). 8/8 pass | `e8079dc` | **PASS** |
| 5 | Load-resilience metrics (non-zero emitted) | `co_metric_reports_concurrency_overhead`; the 8 `codec_ar_admission` admit/shed gates; **+ NEW** `item5_admission_emits_nonzero_metrics_into_rendered_prometheus` (added) | co_metric pass; admission 8/8 pass; **NEW gate renders NON-ZERO:** `admitted_total=2 shed_total{concurrency}=1 inflight=2 vram_reserved_bytes=8192 capacity=2` (real Prometheus render of the live `try_admit` path) | `c495547` / `e2ff596` + new test | **PASS** (gap closed) |
| 6 | Test gaps (chaos/concurrency/fairness/oversized) | `item6_overload_spike_...`; `item6_fairness_one_long_plus_n_short_no_starvation`; `item6_oversized_speak_text_is_typed_413_not_panic` (+3 oversized); `item6_handler_concurrency_...`; `item6_chaos_one_slot_backend_fault_...`; `item6_chaos_wedged_consumer_..._f2` | spike=512 → **admitted=8 (cap 8) shed=504 peak_inflight=8, counters→0**; fairness no-starvation (long 41×, 6 shorts 24 advances); oversized→413, garbage→400; N=12 concurrency peak cohort=12>4; 1/5 slot fault → only that slot Error; wedged consumer → SlowConsumer, loop Ok ~0.24ms. 4 files / 9 tests pass | `4c45b4d` | **PASS** |
| 7 | Batched-path TTFA (TTFA ≪ full-synthesis) | `f6_mux_ttfa_is_far_below_full_synthesis`; `f6_mux_incremental_decode_emit_concat_is_bit_identical_to_whole_body`; `f6_mux_barge_in_after_committed_audio_closes_cancelled` | **TTFA = 58.49 ms vs full-synthesis = 228.17 ms** for 4 concurrent streams (~26%, TTFA ≪ full); incremental-decode concat **bit-identical** to whole-body; barge-in closes Cancelled. 3/3 pass | `0f2d23a` | **PASS** |

**Verdict: 7/7 PASS.** One genuine gap found and fixed minimally (item 5 had no *rendered-metric* gate). One honest scope note on item 2 (below).

---

## Per-item detail (real test output)

### Item 1 — fp16/quant on CUDA (PASS)
Root-cause fix (`59f1adc`): the rejected GQA `attention_bias` input is an all-zero padding mask both drivers
always feed; the CUDA kernel rejects the *slot's presence*, so graph-surgery removes it (bit-identical on CPU).
- `voxtral_q4f16_nobias_runs_on_ort_cuda_byte_identical` (70.16 s): `ORT-CPU (stock q4f16) == ORT-CUDA (no-bias q4f16)`,
  identical transcript, decoder EP = cuda. The quant bytes (MatMulNBits/UINT8 q4) intact → still genuinely q4f16.
- `cohere_fp16_nobias_runs_on_ort_cuda` (29.09 s): `EXACT char-identical: true · de-punct char-sim: 100.0%`, EP=cuda.
- `voxtral_and_cohere_serve_on_ort_cuda_via_registry` (80.99 s): plain `load_model_at(dir, Cuda)` via `waav.json`
  serves BOTH on the CUDA EP, transcript identical to CPU. `_nobias.onnx` + `waav.json` staged in the model dirs.

### Item 2 — full e2e fp16 input-dtype casts (PASS, with an honest scope note)
The input-cast seam (`runtime/precision.rs::feed_float`) is COMPLETE and WIRED into 7 arms (voxtral, qwen3_asr,
funasr_nano, canary, nemotron, parakeet, chatterbox): every float graph-input is now built in the graph's
**declared** dtype (f32 identical / f16 widened via `f16::from_f32`), not a hardcoded f32.
- Unit 4/4: `feed_float_f32_graph_is_bit_identical`, `feed_float_f16_graph_widens` (the f16-input WIDEN branch),
  `feature_inputs_stay_f32_by_name`, `empty_kv_dtype_follows_weight_precision_q4f16`.
- **On-hardware (the load-bearing e2e proof, 365.78 s):** `live_ragged_batched_forward_bit_identical_and_scales`
  drives the real chatterbox model on CUDA through the `feed_float`-built `inputs_embeds` and asserts the produced
  codes are **byte-identical** batched-vs-per-slot across 4 ragged-length slots — i.e. the inputs are CAST through
  the graph-driven seam (not just the output read), and the full forward is byte-faithful. `ragged bit-identity: PASS`.

**Honest scope note (not a gap, a fact):** every *currently-shipping* fp32/q4f16/fp16 export declares these float
*inputs* `F32` (verified live on the on-disk graphs — e.g. voxtral `inputs_embeds` is f32 even in the q4f16
export; ref `WaaV/inferv2/REVIEW/B7-fp16-inputs.md`). So for everything that ships today `feed_float` returns the
identical f32 tensor → ZERO numerical change, and the f16-*input* WIDEN branch is exercised at the unit level
(`feed_float_f16_graph_widens`), not by a shipping checkpoint — there is no f16-input-declared export yet. The
mechanism is correct and ready; it adds correctness for a future f16-declared export. The OUTPUT-read fp16 path
(widening F16→f32) is independently proven byte-faithful on CUDA by item 1's voxtral q4f16 / cohere fp16 runs.

### Item 3 — S2S engine-served + shelfware disposition (PASS)
S2S is now first-class engine-served (`56254bf`): trait `S2sModel` + `LoadedModel::S2s` + `engine.s2s_turn`.
- `s2s_seam_registered_and_object_safe` (GPU-free): pass.
- `lfm2_audio_s2s_via_registry_engine_served` (ONNX CPU, 61.46 s): `load_model_at(lfm2_audio_s2s) → LoadedModel::S2s
  → s2s_turn` is **BYTE-IDENTICAL** to the core `round_trip` (`PASS: engine-served lfm2 S2S turn is BYTE-IDENTICAL`).
- `engine_serves_inprocess_torch_hibiki_s2s_byte_identical_to_standalone` (`--features torch`, CPU-f32, 74.20 s):
  engine-served reply **15360 samples == standalone** (`PASS: ... BYTE-IDENTICAL to standalone`).

**The OTHER named shelfware — WIRED vs DOWN-SCOPED-IN-DOCS** (live-caller audit; 14-crate workspace):

| Component | Disposition | Evidence |
|-----------|-------------|----------|
| **resilience layer** (runtime watchdog: J16 frame-progress + J15 leak reconciler) | **WIRED** | spawned live in `server/src/bin/waav_infer.rs:621` (`spawn_watchdog(state.spine().clone())`), def `lib.rs:421`; also into `CodecArBatcher::new()` lib.rs:227. Live gate `spawned_watchdog_thread_sheds_a_silently_hung_session` proves it sheds a hung session. |
| **scheduler admission** (DutyLedger) | **WIRED (secondary)** | `AppState::try_admit` (`lib.rs:343`) → `engine.admit_bandwidth()` (`engine.rs:1171`) checks DutyLedger bus-saturation. NOTE: the PRIMARY request-path admission is `CodecArAdmission` (server `codec_ar_admission.rs`); DutyLedger is the layered bandwidth-ceiling check on top. |
| **features** (text frontend / SSML / TN) | **WIRED** | `lib.rs:871` `text_frontend::normalize_tts_text(...)` on every `/v1/audio/speech` + native-WS `speak` before synthesis. |
| **backend-api** (`StaticGraph` seam) | **WIRED (seam)** | the pure-Rust backend interface every backend (ORT/Torch) implements; engine model-load returns/consumes it; `parse_ep_request` called in `bin/waav_infer.rs:179`. |
| **dag-CLI** (cascade) | **WIRED (CLI one-shot, not serve-path)** | `RunDag` subcommand → `run_dag_once` (`bin/waav_infer.rs:541`) → `run_cascade(...)`; drives the G11 STT→LLM→TTS accept. Not part of `/v1/audio/speech`. |
| **router** (prefix-affinity) | **DOWN-SCOPED-IN-DOCS** | `waav-infer-router` has ZERO imports/callers in the server; it is a gateway-side fleet-placement helper (GW-5). Pub/test-only from the standalone server's view. |
| **provider** (`waav-infer-provider`) | **DOWN-SCOPED-IN-DOCS** | ZERO imports in the server; it is the gateway adapter (GW-2) that implements the gateway's BaseSTT/BaseTTS seam — not called by the standalone in-process server. |
| **gateway-api** (`waav-gateway-provider-api`) | **DOWN-SCOPED-IN-DOCS** | pure trait/wire-type seam definition; consumed by the provider adapter, never by the server. |

### Item 4 — scheduler hazards closed (PASS)
`e8079dc`. `atomic_admit_and_commit_is_one_critical_section` + `no_clone_admit_matches_clone_based_projection_bit_identical`
+ `concurrent_admit_and_commit_never_over_admits`: **64 threads fired at S=0.8 @ 0.10 duty each → exactly 8 admit**
("exactly 8 admits fit under S=0.8 ... no over-admit under contention"; never 9 = the closed TOCTOU). Migration:
`monotonic_epoch_prevents_double_admit` + `dest_admit_isolates_...` + `leased_buffers_isolate_across_4_concurrent_migrations`
+ `cadence_migration_rejected` + `fault_migration_refused_on_key_mismatch` — a strictly-lower epoch is `StaleEpochReject`-fenced
so exactly one writer ever commits (split-brain prevented). 8/8 pass.

### Item 5 — load-resilience metrics non-zero (PASS — gap found + fixed)
The behavior was tested (`co_metric_reports_concurrency_overhead` + 8 `codec_ar_admission` admit/shed gates, all
pass) but **no gate proved the metrics actually reach a recorder with non-zero values** — the task's explicit ask.
**Minimal fix (the one change made):** added `item5_admission_emits_nonzero_metrics_into_rendered_prometheus` (+ 2
parse helpers) in `crates/waav-infer-server/src/codec_ar_admission.rs` — it scopes a local `PrometheusRecorder`
via `metrics::with_local_recorder`, drives the REAL `try_admit` path (2 admits + 1 cap-shed), renders, and asserts
the item-5 series are present and NON-ZERO. **Live render:**
`admitted_total=2 · shed_total{reason="concurrency"}=1 · inflight=2 · vram_reserved_bytes=8192 · capacity=2`.
Full `codec_ar_admission` suite now 9/9; `cargo clippy -p waav-infer-server --all-targets` clean (no warnings).

### Item 6 — chaos/concurrency/fairness/oversized (PASS)
`4c45b4d`, 4 files / 9 tests:
- `overload_fairness`: spike=512 → **admitted=8 (cap 8) shed=504 peak_inflight=8 → counters back to 0** (no leak);
  fairness `long advanced 41× (budget 40), 6 shorts 24 advances, all Final, no starvation, loop Ok 279µs`.
- `oversized_input` (4): oversized speak.text → typed **413**; zero-len/garbage voice → **400**; garbage utterance →
  typed Error terminal (no panic).
- `chaos_concurrency` (3): N=12 admitted, **peak in-flight cohort = 12 (>4)**; 1/5 slot fault → only that slot Error,
  4 survivors Final; 1/4 wedged consumer → dropped SlowConsumer, 3 survivors Final, loop Ok 237µs.

### Item 7 — batched-path TTFA ≪ full-synthesis (PASS)
`0f2d23a`. `f6_mux_ttfa_is_far_below_full_synthesis` (0.24 s): live print
`[F6] batched-path TTFA: full-synthesis 228.170214ms for 4 concurrent streams; first audio at Some(58.489473ms)
(TTFA ≪ full ✓)`. `f6_mux_incremental_decode_emit_concat_is_bit_identical_to_whole_body`: Σ(mid-loop deltas)++tail
== `decode_audio(full_body)` byte-for-byte. `f6_mux_barge_in_after_committed_audio_closes_cancelled`: pass.
**The honest finding STANDS:** no production codec (csm Mimi / DAC / chatterbox S3Gen) is bit-faithfully
chunk-decodable — incrementality is a model property (`decode_committed_prefix`, default commits nothing →
non-causal codecs are a bit-faithful whole-body no-op), never faked by slicing the decoder.

---

## Fix made
One minimal addition, no commit, HEAD unchanged (`ae98dd6`):
- `crates/waav-infer-server/src/codec_ar_admission.rs` **+83 lines** — the item-5 `item5_admission_emits_nonzero_metrics_into_rendered_prometheus`
  gate + 2 Prometheus-render parse helpers. Closes the one genuine gap (metric *emission* was untested where
  metric-driven *behavior* was tested). `git diff --numstat`: `83  0`. Clippy clean.

## Total wall-time
Live test wall-time (sum of the runs, dominated by the live-GPU model loads):
item 1 ≈ 180 s (70+29+81) · item 2 on-hardware ≈ 366 s · item 3 ≈ 136 s (61+74+seam) · item 5 new gate <1 s ·
items 4/6/7 + unit gates ≈ a few s combined · plus incremental `cargo` builds (`CARGO_BUILD_JOBS=6`).
**≈ 14 min of live test execution** (excluding the one-time backend-torch `--features cuda`/`torch` compiles).
