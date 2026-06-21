# WaaV Infer v2 — Production Limitations RCA (to eliminate, bit-faithfully)

Source: RCA phase of workflow `wf_ed99336f-e4a` (2026-06-20). The RCA agents completed and
produced this inventory; the fix/revalidate agents then died on a **session usage limit**
(resets 20:20 Asia/Kolkata) — so this is the verified-honest limitation list, fixes pending.
LAW for every fix: the REAL bit-faithful fix, **no fallback / approximation / quant**; ragged &
concurrent (different lengths / start times) are the COMMON production case and MUST batch with
full throughput scaling; new batched paths proven BIT-IDENTICAL to per-slot/single reference.

## Blockers (cap production throughput / unrealized batching)

1. **Lockstep batcher NOT wired to live traffic.** The live WS/REST codec-AR path runs
   *single-stream* `serve` under the model mutex — the batched lockstep `Driver`/`step_batch`
   seam is exercised only by tests, **never by real concurrent traffic**. The core batching value
   is unrealized on the actual serving path. → Wire the live serve path through the batched
   cohort scheduler.
2. **Ragged-cohort batching falls back to per-slot** on the BASE chatterbox LM (no `position_ids`
   input → left-padded RoPE diverges). Streams of different lengths / start times do not batch.
   → Re-export `language_model.onnx` with explicit `position_ids` (source `t3_cfg.safetensors`
   local) OR graph-patch; ragged left-pad then bit-identical to per-slot.
3. **STT (all 14 STT arms) + one-shot TTS (kokoro/melo/supertonic) serialize on a single
   `Arc<Mutex<Box<dyn Model>>>`** — concurrent transcribe/synthesize run sequentially (whisper
   crosses RTF=1 at N=16). → Batch concurrent feedforward forwards into one batch-N run, bit-identical.

## Major (correctness-coverage / honesty / unmeasured claims)

4. **Turbo chatterbox HAS `position_ids` yet `step_batch` still forces the equal-context gate** →
   ragged needlessly falls back to per-slot for the turbo arm. Pure-Rust fix, **no re-export**.
5. **No ragged-batch == per-slot bit-identity RED test exists.** The only batched-forward gate
   (`batched_forward_codes_identical_to_per_slot`) uses EQUAL-length prefill; ragged accuracy is
   asserted by prose, not a test. → Add the ragged bit-identity gate.
6. **No real native-S2S / `DuplexStepModel` on GPU.** The only true `SlotBatch` batched seam is
   exercised solely by a `FakeStage` virtual-clock double, so the **≤200 ms full-duplex latency
   gate is MODELED, not GPU-measured.** → Register a real duplex model + measure live.
7. **Codec-AR TTFA is not a true first-token metric.** `serve_codec_ar_stream` runs the ENTIRE AR
   decode loop, then `decode_audio()` ONCE post-loop and slices the buffer — so "first audio
   chunk" == full-synthesis latency (3804 ms) and inter-chunk jitter percentiles are ~0 ms
   buffer-slice artifacts. → Real incremental decode + true TTFA measurement.
8. **"55×@64" thesis does not reproduce on the real path — RESOLVED (re-scoped to the measured
   curve, 2026-06-21).** Re-measured live on GB10 (CUDA EP, equal-context, bit-identical to
   per-slot) up to the doc's claimed B=64 ceiling: the real chatterbox codec-LM batched speedup
   **RISES to a peak then REGRESSES** — 1.12×(B2)/0.99×(B4)/1.39×(B8)/**1.81×(B16 peak)**/1.46×(B32)/
   **0.95×(B64, slower than per-slot)**. Root cause: the exported `language_model.onnx` re-streams
   the full split-KV host↔device every stride (`O(B·max_past·n_layers·2)`), which the synthetic
   GEMV microbenchmark that produced 55×@64 omitted. **The headline is now RE-SCOPED to the
   empirically-measured curve (peak ~1.8× @ B≈16, NOT 55×@64) across INFER_ENGINE.md (§1.1 pts 2/3,
   §4.3), INFER_PERF.md (§0/§3/engine-win #2), INFER_PERF_BENCH.md, INFER_ENGINE_V2.md, and
   INFER_GUIDELINES.md**, pinned by the live re-measurement gate
   `live_headline_batched_scaling_matches_doc_curve` + the single-source-of-truth constants
   `CHATTERBOX_HEADLINE_PEAK_BATCH_SPEEDUP`/`CHATTERBOX_HEADLINE_PEAK_BATCH` (which fail any future
   doc-drift). Accuracy stayed byte-identical (bit-identity proven at width: a 16-row ragged cohort
   batched == per-slot token-for-token). 55×@64 is recoverable only by a device-resident-KV
   re-export (no host re-stream) — the open #1 follow-up; the docs no longer assert it as a shipped
   serving figure. See INFER_PERF_VALIDATION.md §3a/§7.

## #9 Load resilience — graceful degradation under overload (REQUIRED, added 2026-06-21 per user)

A worker must NEVER crash or buffer unboundedly under load. Even if requests spike past what a
worker can serve, or latency explodes, the system MUST queue / shed / backpressure — degrade
gracefully with a clear signal. The 2026-06-20 OOM crashes were the unbounded-buffering failure
mode (the ORT arena cap, commit 950d491, only turns a crash into a clean error — it is the safety
net, NOT the design). Required, bit-faithfully (no correctness loss):
- **Bounded admission queue, not unbounded.** `codec_ar_batcher.rs:57` (egress) + `:80`
  (submission `tx`) are both `UnboundedSender` — the literal flood-to-OOM trap. Make them bounded.
- **Concurrency cap + bounded queue + load-shed.** N active slots (`MAX_SLOTS`); excess requests
  queue up to a BOUNDED depth; when full, reject fast with an explicit server-busy / 429 /
  retry-after (or backpressure the caller) — never admit unboundedly.
- **Deadline-aware admission.** Under latency explosion, drop/reject requests that can't meet their
  deadline (wire the deadline-graded scheduler + `DutyLedger`/`Ceilings` into ADMISSION, not just
  scheduling).
- **VRAM-accounted admission.** Admit a new stream only with memory headroom (`VramAccountant`);
  never admit past the budget.
- **Acceptance gate:** a deliberate request-spike + latency-explosion stress test — the worker
  queues/sheds/backpressures, peak memory stays bounded, NO OOM, NO crash, accepted streams stay
  bit-identical, shed streams get a clear typed signal.

## #10 Integration completeness — NO SHELF-WARE (STANDING RULE, added 2026-06-21 per user)

Every component developed for WaaV Infer (now and earlier) MUST be **optimally integrated into the
live main codebase and serving path** — reachable from the real WS / REST / realtime entry points
and **exercised by a live (non-Fake) integration test** — not test-only, dead, behind a disabled
flag, duplicated, or shadowed by an older path. A component is NOT "done" until it is wired into the
live path AND a live test proves it actually runs there. This is the generalization of the two worst
limitations we found: **#1** (the lockstep batcher was fully built but **never wired to live
traffic**) and **#6** (native-S2S existed but was exercised **only by a `FakeStage` double**).

**Acceptance gate — integration-completeness audit (part of revalidation, every round):** enumerate
EVERY component and trace it from the live server entry points (`ws.rs`, the REST handler, the
realtime/S2S handler, `engine.rs`) — each MUST be invoked on the real path, with a live test
covering it. Audit at least: `codec_ar_batcher` (live codec-AR), `stt_coalescer` (live transcribe),
`tts_coalescer` (live one-shot TTS), the S2S `DuplexStepModel` (live realtime, not FakeStage), the
production spine (`FrameWatchdog`/`LeakWatchdog`/`VramAccountant`), the NaN-reject sampler,
delta-streaming egress, barge-in, the admission machinery (`DutyLedger`/`Ceilings`/deadline-graded +
the #9 bounded queue/shed), and the 16-arm registry. ANY component that is orphaned / test-only /
duplicated / not on the live path is a limitation to wire in (bit-faithfully). Codify this rule in
`INFER_GUIDELINES.md` so it binds future work. "Built" ≠ "shipped": only live-path-integrated counts.

## FOUND + FIXED — ORT first-touch re-entrant-`Once` deadlock (#1 production blocker, 2026-06-21, commit `bbcf663`)

**Symptom.** `engine::tests::gb10_serves_16_concurrent_codec_ar_streams_rtf_under_1` hung FOREVER at
0% CPU (reproduced 4×, killed only by its timeout), RSS flat ~9 MB — i.e. it hung at *first-touch ORT
init*, before any model load. NOT an OOM, NOT the serving-loop concurrency. A live server that lazily
first-touches ORT on its first burst of concurrent requests would hang identically — so this was a real
production blocker, not just a test flake.

**Root cause (gdb-proven, live backtrace of the hung process + a deterministic standalone repro).**
`ort` rc.12 self-deadlocks on **any** dylib-load failure. `setup_api()` enters the
`G_ORT_API`/`G_ORT_LIB` `OnceLock` (held, mid-init), `load_dylib_from_path` fails, and **constructing
the failure `ort::Error`** (`Error::new` → `new_internal` → `ortsys![CreateStatus]`) calls **`ort::api()`
again**, which **re-enters the same in-flight `Once` on the same thread** → `futex_wait` forever. Stack:
`futex_wait → Once::call_once_force → ort::api() (lib.rs:176) → Error::new_internal::{closure}
(lib.rs:291) → load_dylib_from_path::{closure} (lib.rs:107) → try_init_inner::{closure} (once_lock.rs:147)`.
A standalone repro confirmed it deadlocks via **both** entry points: the lazy `Session::builder()` path
(re-enters `G_ORT_API`) AND `ort::init_from(bad)` (re-enters `G_ORT_LIB`) — so `init_from` is NOT a safe
fix. Our old `init_ort()` used `init().commit()`, which only **stores env options** (no dylib load), so
the first *real* touch (`Session::builder`) was the racy/error-prone first load that could trip this.

**Fix (bit-faithful — NO numerics / EP / precision / TF32 / `gpu_mem_limit` / cuDNN change).**
- `waav-infer-backend-ort`: new `ensure_ort_initialized()` — **pre-flight the dylib ourselves** with
  `libloading` (the same crate `ort` uses; a plain `dlopen` that touches no `ort` global) **before** `ort`
  can, converting `ort`'s deadlock-on-failure into a typed `BackendError::Load`; then commit
  `ort::init()` and **force a successful `ort::api()` ONCE, single-threaded**, so `G_ORT_LIB`/`G_ORT_API`
  fill cleanly and no later concurrent first-touch can race the load or hit the error-path re-entry.
  Cached in a process-wide `OnceLock` (idempotent). `OrtModel::load_with` now `?`-propagates it.
- `waav-infer-server`: `Engine::load()` calls `ensure_ort_initialized()?` **as its first step** — the
  production startup fix (eager ORT init before any model load or concurrent serving burst; covers the
  live `serve` path and CLI subcommands via the shared load seam). A bad `ORT_DYLIB_PATH` is now a clean
  boot error, never a hang.
- Regression test `deadlock_regression::preflight_dylib_rejects_bad_path_without_deadlock`
  (watchdog-timer bounded): a bad `ORT_DYLIB_PATH` returns `Err`, never hangs.

**Verified on GB10.** `gb10_serves_16_concurrent_codec_ar_streams_rtf_under_1` COMPLETES + PASSES:
16 concurrent codec-AR streams, `audio=75.440s wall=51.473s RTF=0.6823 ep=cuda`. backend-ort lib suite
24/24 green (incl. live CUDA tiny-graph + `run_bound_*` bit-identity). `cargo clippy --workspace
--all-targets -D warnings` clean.

## Recovery
Resume `wf_ed99336f-e4a` via `resumeFromRunId` after the session limit resets (20:20 Asia/Kolkata):
the RCA agents are cached, so resume re-runs only the fix + revalidate phases.
