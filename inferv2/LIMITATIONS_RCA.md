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
8. **"55×@64" thesis does not reproduce on the real path** (best 1.80×@B8 equal-context / 1.14×
   end-to-end ragged). The validation doc is honest (pass=false) but `INFER_ENGINE.md` /
   `INFER_PERF.md` still assert 55×@64 as the headline lever. → Correct the design docs to the
   measured reality (and let fixes #1–#4 raise the real ragged/concurrent number).

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

## Recovery
Resume `wf_ed99336f-e4a` via `resumeFromRunId` after the session limit resets (20:20 Asia/Kolkata):
the RCA agents are cached, so resume re-runs only the fix + revalidate phases.
