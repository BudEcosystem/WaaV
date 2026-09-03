# WaaV Infer — Observability Surface Map + Perf-Equation Accuracy Audit

Read-only reconnaissance over `waav-infer/` (no code changed). Purpose: ground a layer-by-layer +
kernel profiling build on a **correct** foundation. Owner's hard concern: *"if these equations are
wrong our experiment analysis would also be wrong."* Section B is the load-bearing part.

All citations are `crate/path:line` (absolute root `/home/bud/ditto/waav/waav-infer/`).

---

## A. THE EXISTING-PROFILING MAP

The layered picture the build wants is **request → stage → batch → layer → kernel**. WaaV Infer today
covers the top two-and-a-half layers; **layer- and kernel-level is empty** except for two test-only
component-breakdown hooks.

```
LAYER          COVERED?   WHERE                                          GRANULARITY
request   ✓    Prometheus + per-turn tracing span                  HTTP/WS entry, counters+histos
stage     ~    test-only profile_* hooks (dia2/neutts/voxtral)     CUDA-synced wall per component
batch     ✓    cohort-width histogram + inflight gauge             per serve tick (active_rows.len())
layer     ✗    — none —                                            GAP
kernel    ✗    — none (no CUDA events / nvtx / torch.profiler)     GAP
```

### A.1 — Prometheus metrics (the `/metrics` surface)

**24 distinct `waav_infer_*` series** are emitted from `src/` (README §379 rounds to "23"), plus 2
non-prefixed (`waav_degraded_total`, `waav_ort_ep`) and 2 test-only (`waav_kokoro_*_test`). Recorder
install + endpoint:

- Install: `crates/waav-infer-server/src/bin/waav_infer.rs:593-600` — `PrometheusBuilder::new()`
  `.set_buckets_for_metric(COHORT_WIDTH_METRIC, COHORT_WIDTH_BUCKETS).install_recorder()`. The **only**
  metric with explicitly configured histogram buckets is the cohort width; every other histogram renders
  with the exporter's default bucket set.
- Endpoint: route `crates/waav-infer-server/src/lib.rs:372` `.route("/metrics", get(metrics))`; handler
  at `lib.rs:557` returns `s.metrics.render()` (a `PrometheusHandle`, held in `AppState.metrics`,
  `lib.rs:111`).

| # | Metric | Type | Emit site (file:line) | Measures / cadence |
|---|--------|------|----------------------|--------------------|
| 1 | `requests_total{route}` | counter | lib.rs:821 (speech), 1080 (transcriptions) | per HTTP request, at entry |
| 2 | `errors_total{code}` | counter | lib.rs:1280, ws.rs:769 | per typed error (REST + WS) |
| 3 | `ttfa_seconds{model=kokoro}` | histogram | lib.rs:1027 | **mislabeled**: total one-shot synth wall, not TTFA |
| 4 | `stream_rtf{model=kokoro}` | histogram | lib.rs:1030 | `elapsed/audio_secs` (RTF) for the one-shot REST synth |
| 5 | `stt_latency_seconds{model=whisper}` | histogram | lib.rs:1181 | whole transcription wall |
| 6 | `audio_seconds_total{model}` | counter | lib.rs:1208 (`record_audio_seconds`, lib.rs:1201) | lossless whole-second audio roll-up |
| 7 | `warmup_seconds` | histogram | engine.rs:1680 | boot warmup wall |
| 8 | `ws_sessions_total` | counter | ws.rs:179 | per WS session opened |
| 9 | `model_state{model,state}` | gauge | lib.rs:358,361,486,487 | ready/draining flags |
| 10 | `frame_watchdog_shed_total` | counter | lib.rs:436 | watchdog-shed hung sessions |
| 11 | `leaked_channels` | gauge | lib.rs:448 | J15 leak ledger |
| 12 | `dead_sidecars` | gauge | lib.rs:460 | dead torch sidecars |
| 13 | `quarantine_evicted_total` | counter | lib.rs:474 | quarantine evictions |
| 14 | `quarantine_refused_total` | counter | (quarantine path) | quarantine refusals |
| 15 | `admission_shed_total{reason}` | counter | codec_ar_admission.rs:359,390,415,438,470; codec_ar_batcher.rs:264 | sheds by reason (concurrency/tenant/deadline/low_mem/vram/submit_queue) |
| 16 | `admission_admitted_total` | counter | codec_ar_admission.rs:488 | admits |
| 17 | `admission_inflight` | gauge | codec_ar_admission.rs:612 | live in-flight |
| 18 | `admission_vram_reserved_bytes` | gauge | codec_ar_admission.rs:613 | static KV ledger |
| 19 | `admission_capacity` | gauge | codec_ar_admission.rs:615 | `max_inflight` |
| 20 | `codec_ar_submitted_total` | counter | codec_ar_batcher.rs:255 | submitted to batcher |
| 21 | `codec_ar_serve_deadline_shed_total` | counter | serve.rs:823 | serve-loop deadline sheds |
| 22 | `codec_ar_cohort_width` | histogram | serve.rs:891 | **batch layer**: `active_rows.len()` per tick (buckets 1,2,4,8,16,32,64) |
| 23 | `codec_ar_inflight_streams` | gauge | serve.rs:892 | live multiplexed streams |
| 24 | `decode_crash_reported_total` | counter | codec_ar_batcher.rs:355 | decode crashes |
| + | `waav_degraded_total{component,reason}` | counter | backend-ort/ep.rs:316 | EP degradation |
| + | `waav_ort_ep{ep}` | gauge | backend-ort/ep.rs:311 | active ORT EP |

**Observation for the build:** every series is a request/admission/health counter or a coarse whole-call
latency histogram. There is **no per-stage, per-layer, or per-kernel series**, and only the cohort-width
histogram reaches the *batch* layer. RTF/TTFA are recorded **only on the one-shot REST `/v1/audio/speech`
path** (kokoro) and STT (whisper) — the WS streaming path and the codec-AR batched path emit **no**
latency/RTF series at all (only counters/gauges).

### A.2 — Distributed tracing (W3C, GW-17)

- Carrier: `crates/waav-infer-protocol/src/trace.rs` — `TraceContext` (16-byte trace id, 8-byte span id,
  `sampled`), W3C `traceparent` parse/emit, `child()` re-parents keeping the trace id. Correct, well-tested
  (round-trip, malformed→typed-error, all-zero rejection).
- Engine side: `crates/waav-infer-server/src/otel.rs` — `turn_span(session_id, trace)` opens **one
  `info_span!("infer.turn")` per session**, carrying `trace_id` / `parent_span` / `gw_sampled` as
  structured fields.
- Invoked: `crates/waav-infer-server/src/ws.rs:166` then the **whole session loop** runs inside it via
  `.instrument(turn_span)` (ws.rs:236).

**GAP (important for the build):** the design language is "per-turn / **per-stage** spans," and
`TraceContext::child()` exists to mint stage spans — but in the **engine serve loop there are zero
per-stage spans**. The only caller of `child()` is the *gateway-provider* mapper
`waav-infer-provider/src/ws_map.rs:778-797` (the other side of the hop / its tests), not the engine. So an
operator gets one span covering the entire turn with **no STT / LLM / TTS / codec stage breakdown inside
it**. There is no `tracing-opentelemetry` exporter wired (otel.rs:8 says exporter wiring is "deployment
config"); spans are `tracing` records only. **No `#[instrument]` attributes anywhere in non-test code.**

### A.3 — Per-component profile hooks on models (test-only)

Exactly **two** `pub fn profile_*` methods exist in `backend-torch/src`:

1. `TorchDia2::profile_generate` — `crates/waav-infer-backend-torch/src/dia2.rs:2274`, returns
   `Dia2Profile` (struct at dia2.rs:181). Breaks the AR loop into **components** (not layers/kernels):
   `backbone_ns`, `depformer_ns`, `sample_ns` (CFG + multinomial + host read-back), `host_rt_ns`
   (per-stage H2D `prev` upload), `codec_ns` (Mimi decode), `ar_total_ns`. Methodology is **sound**: each
   region is bracketed by `tch::Cuda::synchronize(dev)` + `Instant::now()` (dia2.rs:2282,2355-2361), so the
   wall is a true synced device time, not async-launch-skewed. Callers: `tests/dia2_rtf_profile.rs`,
   `tests/dia2_precision_ab.rs` — **test-only, not reachable from serve**.
2. `Neutts::profile_step_breakdown` — `crates/waav-infer-backend-torch/src/neutts.rs:1137`, returns
   `StepProfile` (neutts.rs:1940). Per-decode-step **component** split: `lm_head_ns`, `f32cast_ns`,
   `rep_pen_ns`, `sample_ns`, `backbone_ns`. Also CUDA-synced (neutts.rs:1145-1208), with a 3-step warmup
   (`WARMUP=3`, neutts.rs:1162) before measuring — good practice. Caller:
   `tests/cuda_torch_neutts_profile.rs` — **test-only**.

Plus a stage (not component) breakdown in `tests/cuda_torch_voxtral_perf.rs:81-88`: `encode / prefill /
decode` ms + RTF, computed inline in the test (no production hook).

**Characterization:** all three are **component/stage** granularity (a named code region's synced wall),
**never layer- or kernel-level**, and **none is callable from the serve path** — they are diagnostic test
harnesses. They are the closest thing to a profiler the engine has, and the right seam to generalize.

### A.4 — Scheduler perf / roofline model

Defined in `crates/waav-infer-scheduler/src/admission.rs` (re-exported via `lib.rs:31-34`):

| Type / fn | admission.rs:line | What it models | Used where |
|-----------|-------------------|----------------|------------|
| `RooflineClass` | 334 | compute- vs bandwidth-bound (serialize-vs-overlap on the bus) | StageDuty tag; engine calib stages (engine.rs:1842) |
| `Ceilings{tick_secs,bw_bytes_per_s,duty_bound}` | 352 | rated frame period `T_f`, peak DRAM B/s, headroom `S` | the duty denominators; built in engine.rs:1821, codec_ar_admission.rs:288, batcher.rs:656 |
| `StageDuty` | 451 | per-stage `compute_secs` (`T_step`), `bytes_touched`, roofline | duty contributors; `from_calibrated` (516) wires the live DCGM bytes |
| `DutyLedger` | 702 | per-substrate Σ compute_duty + Σ bandwidth_duty + bandwidth-bound count | `admit` (Σ≤S); engine `bandwidth_profile` (engine.rs:1268), `admit_bandwidth` (1741) |
| `calibrate_co_load_profile` | 1058 | live DCGM `DRAM_ACTIVE` scrape → StageDuty set → admit+fold into a DutyLedger | engine `calibrate_bandwidth_profile` (engine.rs:1852) |
| `BatchProfile{Static1,Tens,Wide}` | 1292 | the band a ridge knee is clamped into (`1` / `8..64` / `64..512`) | `SubstrateRoofline::ridge_knee` clamp; `EpCaps` |
| `SubstrateRoofline` | 1380 | `{peak_flops,peak_bw,batch_profile}` → a-priori ridge-point batch knee | `batch_knee` (1470) = `min(ridge_knee, measured B_max)` |
| `masked_bandwidth_duty` (scheduler) | 2327 | idle/masked-slot bus duty fraction (masked ≠ absent) | `MaskedSlotBandwidth::bandwidth_duty`; layered admit |
| `masked_bandwidth_duty` (backend-api) | backend-api/lib.rs:2273 | masked-slot bandwidth **demand (bytes/s)** | `slot_cap` masked-bw cap |
| `cuda_roofline(sm_arch)` | backend-api/lib.rs:847 | per-arch `(peak_flops, peak_bw, BatchProfile)` table | EpCaps / StagePlacer ridge inputs |

**Where it actually bites:** the **bandwidth axis** (`DutyLedger` / `admit_bandwidth`) is the one wired
into live admission, and it is populated **only if a live DCGM `DRAM_ACTIVE` exporter exists**
(engine.rs:1742-1743, 1805-1806). On a box with no DCGM the profile is `None` and `admit_bandwidth` is a
**no-op** (engine.rs:1742). The **compute/FLOP axis** (`peak_flops`, ridge knee) feeds only the *a-priori*
batch knee, which is then `min()`-tightened by the measured `B_max` — so a wrong `peak_flops` biases the
knee but is partly masked by calibration and clamping.

### A.5 — CUDA events / nvtx / torch.profiler / nsys

**None.** Precise grep for `CudaEvent|cuda_event|cudaEvent|nvtx|torch::profiler|ProfilerActivity|`
`record_function|kineto|nsys|ncu|enable_profiling` across `crates/` returns **zero** hits. Every timing in
the codebase is `std::time::Instant::now()` wall, made device-accurate where it matters by an explicit
`tch::Cuda::synchronize(dev)` before/after (the dia2/neutts hooks). **This is the core build gap:** there
is no event-based, layer-resolved, or kernel-resolved timing infrastructure at all.

---

## B. EQUATION-ACCURACY AUDIT

Verdict legend: **✓ correct** · **⚠ correct-but-coarse/heuristic** · **✗ wrong / mislabeled**.

### B.1 — GB10 hardware constants  ✗ (the FLOP peak is wrong/mislabeled) · ✓ (bandwidth is right)

Constants under audit:
- `GB10_PEAK_DRAM_BYTES_PER_S = 273e9` — engine.rs:1285; also `Ceilings::new(0.040, 273e9, 0.8)` at
  codec_ar_admission.rs:288, batcher.rs:656, overload_fairness.rs:30, chaos_concurrency.rs:35.
- `cuda_roofline(121) => (1.0e15, 2.73e11, Wide)` — backend-api/lib.rs:850, documented (lib.rs:833) as
  "**dense fp16/bf16 tensor-core FLOP/s**".

**Real GB10 (Grace-Blackwell, sm_121 / DGX Spark) measured peaks:**

| quantity | code value | real GB10 | verdict |
|----------|-----------|-----------|---------|
| peak DRAM bandwidth | `273e9` (273 GB/s) | **273 GB/s** (256-bit LPDDR5X @ 8.533 Gbps; datasheet) | **✓ correct** |
| peak FLOP/s labeled "dense fp16/bf16" | `1.0e15` (1000 TFLOPS) | **~213 TFLOPS dense FP16/BF16** (measured `mma_f16f16f32`/`mma_bf16bf16f32` ≈ 212.9 TFLOPS) | **✗ ~4.7× too high** |
| (for reference) FP8 dense | — | ~213.7 TFLOPS measured | — |
| (for reference) FP4 dense / **sparse** | — | ~427 TFLOPS dense / **~1000 TFLOPS sparse** | — |

**The bug:** `1.0e15` is the **NVFP4-sparse marketing "1 petaFLOP"** number, not dense FP16/BF16. NVIDIA's
"up to 1 PFLOP" for GB10 holds only for NVFP4 *with sparsity*; remove sparsity → ~500 TFLOPS FP4, and the
**dense FP16/BF16 tensor peak is ~213 TFLOPS = `2.13e14`**. So the constant the doc calls "dense fp16/bf16"
is the FP4-sparse figure, off by ~4.7×. (Sanity check: the table's H100 row is `9.9e14` ≈ H100's real ~990
TFLOPS dense FP16 — so GB10 at `1.0e15` would claim *H100-class dense FP16* on a ~50 W single-die part,
which is physically implausible. The other rows — B200 `2.25e15`, A100 `3.12e14`, Ada `3.3e14` — are
reasonable dense-FP16 numbers, making GB10 the outlier.)

**Why it matters for the build (the owner's exact concern):**
- **MFU is the direct casualty.** `MFU = achieved_flops / peak_flops`. With `peak_flops = 1e15` but the
  true dense-FP16 ceiling `2.13e14`, every MFU reading is **under-reported by ~4.7×** — you would read 10%
  MFU while genuinely at ~47%, and conclude "huge compute headroom" when there is little. Any roofline plot
  drawn on `1e15` puts the compute ceiling ~4.7× too high and the ridge point ~4.7× too far right.
- **Current blast radius is contained but real.** Today `peak_flops` only feeds the a-priori ridge knee
  (B.4), which is clamped to `[64,512]` and `min()`'d with measured `B_max` — so the live admission path
  does not currently mis-decide on it. **But the moment you build the MFU/roofline analysis the owner
  wants, `1e15` is exactly the wrong denominator.**

**Fix (for the build, not applied here):** introduce precision-keyed peaks rather than one scalar —
`GB10: {fp16:2.13e14, bf16:2.13e14, fp8:2.14e14, fp4_dense:4.27e14, fp4_sparse:~1.0e15}`, source the dense
FP16/BF16 ridge from `2.13e14`, and keep `1.0e15` only if explicitly labeled `fp4_sparse`. Bandwidth
`273e9` stays.

*Sources:* NVIDIA DGX Spark / GB10 datasheet (273 GB/s; 1 PFLOP FP4); NVIDIA Developer Forums "Detailed
Compute Performance Metrics for DGX Spark" (measured dense `mma_*` ≈ 212.9 TFLOPS FP16/BF16, 213.7 FP8,
427.3 FP4-dense); Tom's Hardware / Chips and Cheese GB10 analyses (sparse-only 1 PFLOP, ~500 TFLOPS FP4
dense).

### B.2 — RTF computation  ✓ correct (and consistent)

- `stream_rtf` = `elapsed / audio_secs`, `audio_secs = pcm.len()/sample_rate`, `elapsed = t0.elapsed()` —
  lib.rs:1025-1030. This is the **standard RTF = wall_seconds / audio_seconds** (lower is faster, `<1` ⇒
  faster-than-realtime). Confirmed identical convention across the tree: cascade.rs:232 asserts "RTF =
  wall-clock / audio-seconds"; cpu_sweep.rs:124 "RTF = wall / audio-seconds"; perf_bench.rs:179,
  medasr_live.rs:143, voxtral_perf:84, etc. **Dimensionally and conventionally correct.**
- Note the task's parenthetical "(audio_seconds / wall_seconds)" is the **inverse** (a speedup factor); the
  code uses the orthodox `wall/audio`, which is the right ITU/speech-processing RTF. The inverse form
  appears only where explicitly named a throughput, e.g. `audio_s_per_s = audio/wall` (perf_bench.rs:736).
- **Minor labeling nits (not math errors):** `waav_infer_ttfa_seconds` (lib.rs:1027) records the *total*
  one-shot synth wall, not time-to-first-audio (the REST path is non-streaming) — it is a misnamed total;
  and `stream_rtf` on a non-streaming REST path is a whole-request RTF, not a streaming RTF.

### B.3 — Duty / bandwidth equations  ✓ dimensionally + physically correct

- **compute_duty** `= compute_secs / T_f` (StageDuty::compute_duty, admission.rs:555). `[s]/[s]` =
  dimensionless fraction of the frame burned. ✓
- **bandwidth_duty** `= (bytes_touched / bandwidth_bytes_per_s) × (1/T_f)` (admission.rs:565). Dimensional
  check: `[bytes] / [bytes/s] × [1/s] = [s] × [1/s] =` dimensionless. Physically this is *(time to stream
  `bytes_touched` at peak BW) / (frame period)* = the fraction of the frame the shared bus is busy — i.e.
  a per-frame **MBU contribution**. ✓ Equivalent to `bytes_touched / (peak_bw × T_f)` (bytes needed ÷ bytes
  movable per tick). Correct.
- **Admission** `∀ substrate: Σ compute_duty ≤ S` and `Σ bandwidth_duty ≤ S` (DutyLedger::admit,
  argmax-bottleneck `Saturated`). The **serialize rule** (two `BandwidthBound` stages both add to
  `Σ bandwidth_duty`; `ComputeBound` overlaps) is the conservative single-shared-bus model — physically
  sound for one bus. ✓ Boundary validation in `Ceilings::new` / `StageDuty::new` is rigorous (finite, >0,
  `S∈(0,1]`).
- **masked_bandwidth_duty** (scheduler, admission.rs:2327) `= (masked_count·bytes_per_slot)/ceiling ×
  tick_rate` — the **same** formula, so an idle/masked slot charges the bus identically to an active one
  (masked ≠ absent). ✓ Correct and DRY with `StageDuty::bandwidth_duty`.

These are the cleanest equations in the system — correctly dimensioned, validated at the boundary, and
documented to the design §6.4 formula.

### B.4 — Roofline ridge → batch knee  ⚠ standard ridge AI, heuristic knee mapping

- **Ridge** `= peak_flops / (peak_bw × bytes_per_elem)` (SubstrateRoofline::ridge, admission.rs:1427). The
  classical roofline **ridge arithmetic intensity** is `peak_flops / peak_bw` [FLOP/byte] — that part is
  **✓ textbook-correct**. Dividing by `bytes_per_elem` converts FLOP/byte → **FLOP/element**.
- **Knee** `= ⌈ridge⌉` clamped into the `BatchProfile` band, then `min(measured B_max)`
  (ridge_knee/batch_knee, admission.rs:1437,1470). **⚠ heuristic:** identifying "FLOP-per-element at the
  ridge" with "optimal batch size (stream count)" has **no first-principles derivation** — it is a coarse
  proxy. Dimensionally the quantity is FLOP/element, not a count of streams. The code is **honest about
  this** (lib.rs:834 "coarse by design… never an exact perf model"; clamped to a band; tightened by the
  measured `B_max`). Acceptable as an a-priori hint **only because** it is clamped + calibration-tightened.
  - Numeric consequence of B.1's wrong FLOP: with `1e15`, `ridge = 1e15/(2.73e11×2) = 1831 → clamp →
    **512**`; with the correct `2.13e14`, `ridge = 390 → clamp → **390**`. So the wrong constant inflates
    the a-priori Wide-band knee 512 vs 390 (~1.3×). Masked in practice by the `min(B_max)`, but it *is* a
    wrong input flowing into a knee.
- **slot_cap** `= min(knee, (vram_free−weights)/kv_per_slot, masked_bw_slots)` (backend-api/lib.rs:2258).
  Sound: tightest of three independent caps, `saturating_sub` + `max(1)` guard underflow/div0. ✓

### B.5 — `masked_bandwidth_duty` name collision  ✗ misleading (dimensional mismatch across crates)

There are **two** functions named `masked_bandwidth_duty` with **different dimensions**:
- scheduler/admission.rs:2327 → a **duty fraction** (dimensionless; divides by `peak_bw`, ×`tick_rate`). ✓
- backend-api/lib.rs:2273 → `captured_cohort × bytes_per_slot_step × tick_rate` → **bytes/s** (an absolute
  bandwidth **demand**; it does **not** divide by peak bandwidth). The name says "duty" but it returns a
  rate, **not** a fraction (the doc string does say "bytes/s").

Not a math error in isolation (the backend-api one is a correct bytes/s demand feeding `slot_cap`), but the
**same name for a fraction and a rate is a footgun** for an MFU/MBU build — anyone wiring "duty" expecting a
`[0,1]` fraction will silently get a `bytes/s` magnitude. Recommend renaming the backend-api one to
`masked_bandwidth_demand_bytes_per_s` before building analysis on top.

### B.6 — Hardcoded magic constants where measured/derived values belong

| constant | site | claims | reality |
|----------|------|--------|---------|
| `compute_secs: 0.010` / `0.005` | engine.rs:1841,1848 | "the per-tick step cost (the warmup pass measured it)" | **✗ hardcoded** 10 ms / 5 ms — the warmup pass does **not** feed a measured value here; the `T_step` going into the live bandwidth calibration is a guess. Directly contaminates `DutyLedger` compute-duty if that axis is ever admitted-against. |
| `RATED_STREAM_SERVE_SECS = 0.5` | codec_ar_admission.rs:97 | "Derived from the rated Ceilings tick × a typical stride count" | **⚠ hardcoded** `0.5` — *not* actually derived from `Ceilings` in code; a conservative default the deadline projection (`ahead × 0.5 s`) multiplies. Control-plane only, conservative, low-risk, but a magic number. |
| `one_frame_ms() = 100` | codec_ar_admission.rs:621 | "one AR frame's worth of retry-after" | **⚠ inconsistent** with the rated `Ceilings` tick `0.040 s` (40 ms). The "one frame" retry hint is 100 ms while the rated frame period is 40 ms. Coarse retry hint only — not load-bearing, but two different "one frame" values coexist. |
| `T_f = 0.040 s` (frame period) | engine.rs:1821, admission.rs:288 | rated GB10 frame budget | **⚠ assumed**, not per-model. 40 ms = 25 Hz; real codec frame rates vary (Mimi = 12.5 Hz = 80 ms/frame). It is a rated deadline budget, reasonable as a policy default, but the *actual* per-model frame period differs and a per-model profiler should use the model's true `T_f`, not this scalar. |
| `S = 0.8` (duty_bound) | everywhere `Ceilings::new(…,0.8)` | headroom fraction | ✓ a policy choice (20% jitter headroom), not a physical constant — fine. |

---

## BOTTOM LINE FOR THE BUILD

1. **Fix B.1 first.** The single most consequential error is `cuda_roofline(121).peak_flops = 1.0e15`
   labeled "dense fp16/bf16". The real GB10 dense FP16/BF16 ceiling is **~2.13e14 (213 TFLOPS)**; `1e15` is
   the FP4-**sparse** marketing number. Build MFU/roofline on `2.13e14` (precision-keyed), keep `273e9`
   bandwidth (correct). Without this, every MFU/roofline conclusion is ~4.7× off — exactly the owner's
   failure mode.
2. **The duty/bandwidth math (B.3) is correct** and is the right basis to reuse; the **ridge→batch-knee
   mapping (B.4) is a labeled heuristic**, not a perf model — don't promote it to "the perf model."
3. **The profiling surface stops at the batch layer.** Layer- and kernel-level is empty; the only
   component-level hooks (dia2/neutts `profile_*`) are test-only but use the **right methodology**
   (CUDA-sync + warmup) and are the correct seam to generalize into a serve-callable, CUDA-event-based,
   layer/kernel-resolved profiler.
4. **Per-stage tracing is designed but not emitted** in the engine — only the per-turn span exists; wiring
   `TraceContext::child()` stage spans into the serve loop is a near-free win.
5. **Rename the backend-api `masked_bandwidth_duty`** (it returns bytes/s, not a duty) and **replace the
   hardcoded `compute_secs` 0.010/0.005** with the warmup-measured `T_step` before any analysis trusts the
   compute-duty axis.
