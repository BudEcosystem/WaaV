# WaaV Infer — Enterprise-Readiness Review · SYNTHESIS (verified findings)
Maintained by the lead reviewer; entries here are GREP/READ-VERIFIED, not raw agent claims.

## VERIFIED — Scheduler integration gap (refines 02-scheduler.md CRITICAL-1)
The live admission path = `server/codec_ar_admission.rs` (bounded semaphore + VramAccountant + deadline
via Ceilings + 429 shed) — load-resilient (#9 proven), but SIMPLE.
- WIRED/live (so their bugs are real): DutyLedger (engine.rs admit_bandwidth), SlotTable (duplex/engine/lib),
  VariableStrideLoop (dynamic_fr), Ceilings, lifecycle FSM, reconnect governor, migration, rollout/LoRA.
- UNINTEGRATED shelf-ware (only referenced inside the scheduler crate): LayeredAdmission, CohortPlanner,
  RingKv, TierExecutor, KvFirewall — the advanced layered-admission/cohort/tier/KV-firewall scheduling the
  architecture advertises is NOT on the live path. → Decide in Phase E: wire it, or honestly down-scope the
  "sophisticated scheduler" claim. Either way the docs must match reality.
- Live-hazard scheduler bugs to fix (DutyLedger IS used): admission.rs:824 clones full substrate map/call;
  admission.rs:815 admit→add TOCTOU over-admit; migration.rs:333 stale-epoch guard prose-only (no dest reject).

## VERIFIED — Server/data-plane CRITICALS (from 04-server-integration.md, re-confirmed by lead)
These materially break the enterprise claims (concurrency, low-latency-under-load, slow-client isolation,
operational drain). All read-confirmed. FIX in Phase E (bit-faithful; accepted streams stay identical).

1. [CRITICAL] SLOW-CONSUMER HEAD-OF-LINE BLOCK. `codec_ar_batcher.rs:358 bounded_send` does
   `std::thread::sleep(2ms)` up to EGRESS_BACKPRESSURE_BUDGET=10s (`:79`) ON the single shared mux thread
   (`serve.rs:597 serve_codec_ar_multiplexed_inner`, spawn_blocking, one thread). One full/slow egress
   stalls ALL active slots ≤10s. → FIX: egress send on the shared loop must be NON-BLOCKING (try_send;
   on Full → drop-that-slot's-chunk + mark SlowConsumer immediately, OR a per-stream forwarder task the
   loop hands to via try_send). Bound MEMORY without blocking the loop thread. Add a no-head-of-line gate.

2. [CRITICAL] CODEC-AR CONCURRENCY CAPPED AT 4. WS `speak` (ws.rs:295) holds `try_admit()`'s
   max_concurrency=4 semaphore (lib.rs:83/179) for the whole stream AND submits to the batcher GATE #9
   (MAX_SLOTS=24/MAX_ADMIT=8). Outer gate binds → effective codec-AR concurrency = 4. The 24-slot batcher
   never sees >4 from the live API; gb10_serves_16_concurrent passes only by calling the batcher directly.
   → FIX: codec-AR is gated SOLELY by the batcher's GATE #9 (it IS bounded+VRAM+deadline); the WS/REST
   codec-AR path must NOT also hold the flat outer semaphore. Keep try_admit for one-shot/STT paths.
   Raise max_concurrency default. Add a test that drives concurrency THROUGH the WS/REST handler, not the
   batcher directly.

3. [CRITICAL] BATCHED-PATH TTFA = FULL SYNTHESIS. Single-stream `serve_codec_ar_stream` streams
   incrementally (decode_audio_stream, serve.rs:401); the multiplexed loop emits only at slot completion
   (`drain_finished_stream` serve.rs:776). Concurrent streams get first audio at END → TTFA == full-synth
   latency under load (the #7 fix never reached the batched path the handlers use). → FIX: incremental
   per-tick decode+emit in the mux loop (decode_audio_stream per slot as codes are produced), bit-identical
   concatenation. Add a batched-TTFA gate (first-chunk << full-synth under N concurrent).

4. [HIGH→CRIT] DRAIN DOESN'T GATE ADMISSION. control_drain → control().apply(Drain) (lib.rs:618);
   try_admit (lib.rs:253) checks admit_calibrated+admit_bandwidth but NOT the drain/admit_ok state →
   POST /v1/control/drain returns 200, /readyz stays ready, worker keeps admitting. Only SIGTERM drains.
   (confirm control::apply Drain arm calls engine.begin_drain + wire try_admit to refuse on drain.)
   → FIX: try_admit must consult control().admit_ok(); drain must flip it + /readyz; add a drain-rejects-new test.

## Also from 04 (HIGH, to fix): reconnect-storm cap admit_reconnect has zero callers (dead defense,
control.rs:417); load-resilience layer emits ZERO metrics (no 429/shed/inflight/VRAM/queue-depth — blind);
WS speak silently drops clear/flush/session.update control frames (ws.rs:339); control plane ~63% endpoints
unreachable (operationally blind); S2S realtime route is a stub; cascade is CLI-only.

## VERIFIED — Core/accuracy CRITICALS (from 03-core-backend.md, re-confirmed)
- [CRITICAL/SYSTEMIC] fp16/q4f16 OUTPUT extraction broken. Arms use `TensorData::as_f32()` (→None for F16,
  backend-api/lib.rs:74) instead of `to_f32_vec()` (widener, :100). 47 `as_f32()` vs 5 `to_f32_vec()` in
  core arms; every arm uses the bad pattern. Arms with real F16 outputs (voxtral q4f16 :188, supertonic
  :288/:867, canary :265, chatterbox vocoder :940) → hard-error or silent-empty on fp16. Advertised
  precision is unverified/broken. → FIX: to_f32_vec() at every graph-OUTPUT read; add a per-arm fp16
  smoke/accuracy gate (currently zero fp16 live coverage).
- [CRITICAL] S2S `CodecArDuplexModel` is a SYNTHETIC SCAFFOLD, not a model (contradicts session #6 "real
  native-S2S"). duplex_codec_ar.rs:162 hash-folds user codes into chatterbox text tokens (a*31+c mod 4096),
  no acoustic encoder / no trained EoT head; model.rs registers NO s2s arm (unloadable). #6 made it
  GPU-MEASURED but the conditioning is fake. → DECIDE Phase E: honestly down-scope ("S2S = benchmark
  scaffold / roadmap", fix INFER docs + the #6 memory) OR build a real S2S (large, out-of-session). Do NOT
  let it ship advertised as real native S2S.
- [CRITICAL] funasr_nano silently drops KV writes on overflow (stt/funasr_nano.rs:193) → compounding
  transcript corruption, zero signal. → FIX: error/flush on overflow, never silent-drop.
- [CRITICAL] qwen3_asr hardcodes 2-byte fp16 stride on the embed table (stt/qwen3_asr.rs:110,115) → garbage
  on fp32/bf16 export. → FIX: stride from actual dtype.
- [CRITICAL] voxtral truncates the 39-tok prompt when n_audio<39 (stt/voxtral.rs:235) → corrupt scaffold on
  the short/streaming clips it targets. → FIX: pad, don't truncate the prompt scaffold.
- [HIGH] No repetition guard: voxtral cap 8192, canary cap 1024, funasr — degenerate decode runs to cap
  silently. chatterbox: 1 hardcoded voice, ignores voice/speed, cloning unwired (tts/chatterbox.rs:1160).
- VERIFIED-GOOD (don't regress): encdec AR loop, ORT backend (TF32-off, bounded arena, int8-CUDA refusal,
  dylib-deadlock defense, F16 output extraction), registry quant-stamp gate, chatterbox/supertonic bit-id.

## RUNNING TALLY (verified criticals): server 4 · scheduler (shelf-ware + DutyLedger/migration hazards) ·
## core 5 (+systemic fp16). Pending: runtime hot-path + cross-cutting landmine reviews; heavy live gates.

## VERIFIED — Runtime CRITICALS/HIGHs (from 01-runtime.md, re-confirmed) — and the DOMINANT theme
**DOMINANT THEME (triple-confirmed: runtime + scheduler + server-control):** a LARGE fraction of WaaV
Infer is `pub`/unit-tested SHELFWARE with no live callers. The advertised "non-optional" GB10 resilience &
sophisticated-scheduling guarantees are INERT. Live path = a SMALL, genuinely-good spine (codec-AR
batcher+admission+serve-loop+driver+watchdog-subset+model arms). Gap between advertised architecture and
what runs is the #1 enterprise-readiness issue.
- Resilience layer UNINTEGRATED (verified 1-2 non-self refs each): accel, cuda_graph, graph_fallback,
  paged_kv, prefix_cache (+ kernel_discipline, cell, reasoning, cfg_batch, dynamic_fr, AcousticDelayRing,
  numerics::sample_token, watchdog legs InputFirewall/DeadLetterSink/SourceRateLimiter/RecycleGate). →
  H4 crash-isolation / J1 poison-firewall / L12 long-form-KV are NOT live. DECIDE Phase E: wire the
  high-value (crash-isolation, paged-KV long-form) or honestly down-scope the resilience claims.
- [CRITICAL] (dup of server#1, CONFIRMED ×2) slow consumer blocks whole loop ≤10s — serve.rs:776 sink
  synchronous on mux thread → codec_ar_batcher.rs:358 bounded_send thread::sleep. (=F2)
- [CRITICAL] S2S step() maps model-out by ITERATOR POSITION; cross-tenant guard is debug_assert (release
  no-op) — duplex_codec_ar.rs:297/302 expect(). Release: wrong-slot output = cross-tenant audio bleed +
  expect()-panics the shared loop. Gated behind S2S=scaffold, but FIX the pattern: index stepped/active by
  slot_id (HashMap lookup), runtime-check (not debug_assert) the invariant.
- [HIGH] Disconnected consumer keeps consuming step_batch GPU for the whole utterance — serve.rs:740 active
  filter is `!done` only, never hung_up/cancelled. Disconnect storm pins the cohort on dead work. → FIX:
  drop hung_up slots from active_rows (free GPU immediately). (folds into F2)
- [HIGH] O(active²) per tick — serve.rs:755/1021 linear find(|s| s.slot==slot) per output row → scaling
  cliff at high concurrency. → FIX: index by slot.
- [HIGH] precision.rs narrows every non-F16 KV dtype→F32 (voxtral.rs:31) → crashes next q4f16/bf16 on CUDA
  (folds into F4 systemic fp16/dtype).
- [HIGH] ComputeLease leaks duty on Drop (scheduler/lease.rs:63 no Drop) → starves reasoning pool. (=F9-adj)
- VERIFIED-GOOD: live serve loops / driver / arstep / watchdog-subset are high quality (typed-error-not-panic,
  RAII lease-free on every ?, masked≠absent live-gated); watchdog arithmetic over-defended (panic-poison
  hypothesis REFUTED).

## VERIFIED — Cross-cutting sweep (05) — robustness GOOD, integration BAD
LIVE-PATH ROBUSTNESS IS STRONG (anti-FUD): 0 client-reachable panics on the server front door; exactly
ONE real `unsafe` tree-wide (backend-ort dlopen preflight, correct); 0 blocking-in-async; 0 live
todo!/unimplemented!/stub; bounded decode loops; poison-recovering locks. The serving path will NOT crash
on hostile input. (So fixes are about correctness/latency/integration, not crash-hardening the front door.)

NEW live finding (Tier 1/2): coalescer UNBOUNDED queues — server/tts_coalescer.rs:51/70 + stt_coalescer.rs:46/65
use `unbounded_channel`, bounded only INDIRECTLY by the admission semaphore (asymmetric w/ the hardened
codec_ar batcher). #9 bounded codec-AR but NOT the one-shot-TTS/STT coalescers → still a flood vector.
→ FIX (F5b): bound the coalescer submit queues + load-shed 429, like codec_ar_batcher.

DEAD/UNINTEGRATED CRATES (definitive, Cargo-graph + symbol-grep):
- waav-infer-router — PURE shelf-ware, ZERO dependents (a whole prefix-affinity/failover engine nothing calls).
- waav-infer-provider — pure shelf-ware, zero `use` outside tests.
- waav-gateway-provider-api — reachable only via provider+router (both dead) → server-unreachable.
- waav-infer-dag — only via CLI `run-dag` + tests, NOT request handlers.
- waav-infer-features — 5/7 modules dead (incl transport_egress — the live egress is dag/terminal.rs);
  only `bias` + `stable_span` consumed. (Trap: TWO `StreamingEncoderCache` types — features vs prefix_cache.)
- backend-api — ~900 lines pub policy API (StagePlacer/Relay/Shm/credit-pool/ngram-guard), zero callers.
WIRED+clean: components, protocol, backend-ort, core (16 arms), scheduler (lease/reconnect/lifecycle subset).
Other live landmines to harden: encdec.rs:563 slice_rows no `ai<b` guard (panic on cohort bug); diarize.rs:209
slice trusts claimed shape (panic on malformed graph out); qwen3/funasr out-of-range token→silent zero embed
(silent wrong transcript, folds into F5). Sidecar idle-zombie scan (watchdog.rs:2835) + poison/dead-letter
quarantine: built, NO production poller (resilience shelfware — fold into the wire-or-downscope decision).

## ====== REVIEW COMPLETE (6/6 + coverage). NEXT: heavy-gate perf data → Phase E fixes → Phase C/D live ======
