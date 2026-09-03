# WaaV Infer — Phase E Fix Plan (prioritized, bit-faithful)
Rule for every fix: accepted streams stay BIT-IDENTICAL (control-plane/latency/precision-correctness only,
no numerics change); every fix lands with a VERIFY gate (a new/repaired test that fails before, passes after);
no skip-to-pass / no shelf-ware. Build/verify only when the GPU + build lock are free (heavy gates done).

## STATUS (live)
- **F1 ✅ DONE (compiles, cargo check 0)** — ws.rs drops the `try_admit` permit right after `batcher.submit`
  (3 edits: add drop post-submit, remove deadline-arm drop, remove post-loop drop); lib.rs REST drops
  `_permit` post-submit; `max_concurrency` default 4→64. Codec-AR now gated SOLELY by GATE #9 (24). Verify
  test (≥16 concurrent THROUGH the handler) lands with the serving cluster commit.
- **F2 ✅ DONE (compiles, cargo check 0)** — `bounded_send` (codec_ar_batcher.rs) is now NON-BLOCKING (removed
  the `thread::sleep` ≤10s + `EGRESS_BACKPRESSURE_BUDGET`); `EGRESS_CHANNEL_DEPTH` 64→8192 (holds a whole
  utterance so a real-time reader is loss-free → bit-identical; a consumer that fills its buffer is a typed
  SlowConsumer). The single `codec-ar-mux` loop thread NEVER parks on a slow consumer ⇒ one slow tenant can't
  stall the other 23. Verify test (N streams, 1 wedged consumer, others keep cadence) lands with the cluster.
- **F4/F5 — arms fixes: delegated to a focused agent (running)**, editing only core/ arm files (fp16 widener
  as_f32→to_f32_vec at F16-output sites + funasr/qwen3/voxtral/encdec/diarize corruptions) → REVIEW/09-arms-fixes.md.
- **#4 (disconnect keeps stepping step_batch, HIGH) → Tier 2.** serve.rs:517-519 shows it's a DELIBERATE
  cadence tradeoff (a hung_up slot keeps draining so the lockstep batch shape is unperturbed); cost is bounded
  (one utterance then freed). Proper fix = a mid-stream `egress.is_closed()` probe on AdmitTicket to drop a
  vanished consumer's slot early (masked≠absent makes this bit-safe). Deferred — not CRITICAL.
- **F3 ✅ DONE (compiles, cargo check 0)** — `try_admit` now refuses when `control().admit_ok()` is false
  (before any permit, no leak; typed 503/ModelNotReady); `/readyz` (health_ready) returns not-ready when
  draining so the LB STOPS routing (not just 503-after-route); `control_drain` sets the model_state gauge
  (alert-pack observability). Drain now works end-to-end: stop routing + refuse new + live streams finish.
- **NEXT (GPU pass):** write + run the serving-cluster VERIFY tests — (a) F1: ≥16 concurrent codec-AR THROUGH
  the ws/REST handler admit >4; (b) F2: N streams + 1 wedged consumer → others keep cadence, loop never
  stalls, wedged → SlowConsumer; (c) F3: POST drain → next admit 503 + /readyz not-ready + live stream
  finishes. Then collect F4/F5 agent, combined build + clippy, commit. THEN Phase C (per-model profiling)
  + Phase D (chaos/concurrency).

## TIER 1 — CRITICAL, fix this session
- **F1 ✅ · Codec-AR concurrency cap → use the batcher gate as sole admission** (server#2).
  WS `speak` (ws.rs:295) + REST (lib.rs:741) must NOT hold the flat `max_concurrency=4` `try_admit` permit
  across a codec-AR/batched stream — the batcher GATE #9 (bounded+VRAM+deadline, MAX_SLOTS=24) IS the
  admission. Keep `try_admit` for one-shot/STT/buffered paths. Raise `max_concurrency` default (≥ batcher).
  VERIFY: a NEW test that drives ≥16 concurrent codec-AR streams THROUGH the ws/REST handler (not the
  batcher directly) and observes ≥16 admitted at RTF<1.
- **F2 · Slow-consumer must NOT block the shared loop** (server#1).
  `codec_ar_batcher.rs:358 bounded_send` `thread::sleep`s ≤10s on the single mux thread (serve.rs:597).
  Make the shared-loop egress send NON-BLOCKING: `try_send`; on Full → immediately mark `SlowConsumer` +
  drop that slot's chunk (bound MEMORY, not the loop), or hand egress to a per-stream forwarder task the
  loop feeds via `try_send`. Zero `thread::sleep` on the loop thread.
  VERIFY: NEW test — N streams, one consumer never drains; assert the other N-1 keep their per-tick cadence
  (no >Xms stall) and the slow one gets a typed SlowConsumer; loop never blocks.
- **F3 · Drain must gate admission** (server#4).
  `try_admit` (lib.rs:253) must consult `control().admit_ok()`; `Drain` flips it + `engine.begin_drain()` +
  `/readyz`→not-ready. VERIFY: NEW test — POST drain → next admit returns typed 503/drain, live streams finish.
- **F4 · fp16/q4f16 OUTPUT extraction** (core#5, systemic).
  Replace `TensorData::as_f32()` → `to_f32_vec()` at every graph-OUTPUT read in F16-emitting arms (voxtral
  :188, supertonic :288/:867, canary :265, chatterbox vocoder :940, + audit all 47 sites — input/always-F32
  sites stay). VERIFY: per-arm fp16 smoke gate (load fp16 variant → non-empty, finite, ~matches f32 within
  fp16 tol) — currently ZERO fp16 live coverage.
- **F5 · Per-arm output corruption** (core#2/3/4).
  funasr_nano:193 KV-overflow → error/flush not silent-drop; qwen3_asr:110 embed stride from real dtype;
  voxtral:235 PAD (not truncate) the 39-tok prompt when n_audio<39. VERIFY: targeted unit test per arm.

## TIER 2 — HIGH
- **F6 · Batched-path incremental TTFA** (server#3). Per-tick `decode_audio_stream` + emit in the mux loop
  (serve.rs:772 region), bit-identical concatenation. VERIFY: batched-TTFA gate (first-chunk « full-synth at N≥8).
- **F7 · Load-resilience METRICS** (server HIGH). Emit 429/shed rate, in-flight, queue-depth, reserved-VRAM,
  backpressure, per-model latency, WS errors → `waav_infer_*`. VERIFY: metric-presence test + scrape.
- **F8 · Repetition / decode-cap guards** (core HIGH). voxtral(8192)/canary(1024)/funasr degenerate-decode
  detection + sane caps. **chatterbox** honor `voice`/`speed`, wire cloning or reject typed.
- **F9 · Scheduler live hazards** (sched CRIT, DutyLedger IS live). admission.rs:824 stop full-map clone/call;
  admission.rs:815 single-critical-section admit+commit (no TOCTOU); migration.rs:333 real dest-side epoch reject.
- Reconnect-storm cap wire `admit_reconnect` into ws::session (control.rs:417 dead); WS `speak` ack/handle
  clear/flush/session.update (ws.rs:339); WS handshake/control-frame rate+size budget (slowloris, ws.rs:42).

## TIER 3 — HONEST DOWN-SCOPE (document truthfully; do NOT ship as advertised)
- **S2S is a benchmark scaffold, not a real model** (core#1). Hash-conditioned chatterbox backbone, no
  trained head, unregistered. → Correct INFER_ENGINE/INFER_SPEC + the #6 memory: "native S2S = scaffold /
  roadmap". Real S2S = out-of-session (needs a trained duplex model).
- **Scheduler advanced machinery unintegrated** (sched#1): LayeredAdmission/CohortPlanner/TierExecutor/
  RingKv/KvFirewall not on live path. → Document as roadmap OR wire (large). Don't claim sophisticated
  multi-tenant scheduling beyond the live bounded gate.
- **Test gaps** (07): add chaos/fault-injection, fairness/starvation, oversized-input, drain, slow-consumer,
  WS-path-concurrency regression gates (Phase D exercises these live first).

## EXECUTION ORDER
1) Wait: runtime + cross-cutting reviews land; heavy gates finish (free GPU/build).
2) Fan out TIER-1 fixes (F1-F5) on non-overlapping files in parallel (worktree isolation), each with its
   VERIFY gate; build+test memory-safe (source gb10-env.sh, --test-threads bounded, timeout-wrapped).
3) Phase C real per-model profiling (06 plan) + Phase D chaos/concurrency (07 plan) on the freed GPU.
4) TIER-2 + TIER-3 docs. Re-run full regression + heavy gates. Final enterprise-readiness verdict.
