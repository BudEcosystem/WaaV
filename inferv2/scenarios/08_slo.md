# 08 — Request-prioritization / SLO / QoE / latency-vs-throughput scenarios

> Family: maximizing user experience in a **pipelined, multi-stage, multi-user** realtime voice engine. Every scenario is grounded in `INFER_ENGINE.md` (§4.4 isochronous constraint, §4.5 prefill firewall, §4.6 free token pacing, §6 scheduler/SLO/duty-ledger) and the production failure catalog (L1 prefix-cache, L3 VoxServe binary-viability, L9 Niyama graceful relegation, L10 KV-length-aware firewall, H8 wall-clock aging).
>
> **The competing objectives** (no single number is "the" SLO):
> - **TTFA / initial-response** (time-to-first-audio; the felt "is it alive?" latency).
> - **End-to-end latency** (mouth-to-ear for a full turn, esp. cascaded STT→LLM→TTS).
> - **Aggregate throughput** (streams/sec the box sustains — the batch/$ metric).
> - **Per-session throughput** (does THIS stream keep emitting ≥ realtime, RTF<1).
> - **Per-session latency / ITL jitter** (inter-token / inter-frame stability, p99).
> - **Frame-deadline** (the hard isochronous wall: a step overrunning the frame period = audible underrun).
>
> **The governing physics:** voice is consumed at ~3.3 tok/s and emits one frame per fixed tick → a frame-synchronous loop paces at exactly the consumption rate **for free** (§4.6, Andes Token Pacer). Once a frame is delivered *before* its playback deadline, further latency on it is **worthless** (VoxServe binary streaming-viability, L3) → don't over-serve a session that's already safe; spend that budget on a session at risk, or piggyback Batch (Sarathi). Realtime > Batch is a per-stage priority. **Graceful degradation is the PRIMARY overload tool** (Niyama 95%@50%-overload, BrownoutServe quality-brownout); **hard reject only at true saturation** (L9). The binding stage is usually **NOT the AR stage** — it's the codec/CFM/vocoder bottleneck (§6).

---

## SIMPLE — single objective, single/few sessions

### SLO-1 — First-audio is the felt latency, not total turn time
- Level: Simple
- Pipeline: any TTS (AR or CFM)
- Axes: slo:TTFA, priority:realtime, qoe:perceived-latency
- Scenario: A user sends one sentence to synthesize; the whole clip takes 1.2 s to generate but the first 80 ms frame is ready in 180 ms.
- System must: Stream the first frame the instant it exists (delta egress, §4.6); optimize the schedule for TTFA-to-first-frame, not for whole-clip completion time.
- If mishandled: Buffering the whole clip before sending makes a fast engine feel like a 1.2 s laggy one.

### SLO-2 — RTF<1 is the per-session survival condition
- Level: Simple
- Pipeline: any streaming TTS/STS
- Axes: slo:session, priority:realtime, frame-deadline
- Scenario: One live call; the model must emit one 80 ms frame per 80 ms wall tick to avoid an underrun.
- System must: Meter per-step wall time against the frame budget (`step_time + overhead ≤ T_f − jitter_margin`, §4.4); keep steady-state RTF below 1 with margin (~70-80% utilization).
- If mishandled: RTF creeping above 1 silently accumulates lag until the playback buffer drains and the user hears a gap.

### SLO-3 — Don't over-serve a session that's already safe (binary viability)
- Level: Simple
- Pipeline: any streaming
- Axes: slo:session, priority:realtime, throughput
- Scenario: A session's next frame is already 200 ms ahead of its playback deadline; the scheduler could rush it further or move on.
- System must: Treat streaming-viability as **binary** (VoxServe, L3) — once delivered-in-time, extra earliness is worthless; stop spending budget on it and give the slack to an at-risk session or a Batch job.
- If mishandled: Greedily minimizing every session's latency wastes GPU that a near-deadline session or backlog batch needed.

### SLO-4 — Frame pacing is free, so don't run faster than playback
- Level: Simple
- Pipeline: AR codec-TTS
- Axes: slo:session, throughput, priority:realtime
- Scenario: A single TTS stream could generate at 11 tok/s but audio is consumed at ~3.3 tok/s.
- System must: Pace at the consumption rate by construction (one frame/tick, §4.6); the leftover GPU is reclaimable for other streams or Batch, not burned over-generating ~2.3× surplus.
- If mishandled: Continuous-batching-style over-generation wastes ~2.3× GPU and produces audio nobody is listening to yet.

### SLO-5 — Playback buffer absorbs jitter; protect its cadence, don't drop frames
- Level: Simple
- Pipeline: any streaming egress
- Axes: slo:session, qoe:jitter, priority:realtime
- Scenario: One step occasionally spikes from 9 ms to 14 ms but stays under the 80 ms budget.
- System must: Rely on the client playback buffer to smooth sub-budget jitter (VoxServe/TokenFlow cadence model, L9); never frame-drop for a transient that the buffer can hide.
- If mishandled: Reacting to harmless jitter with drops or resets turns a smooth stream choppy.

### SLO-6 — Bound the egress queue; a slow consumer must not hoard audio
- Level: Simple
- Pipeline: any streaming TTS
- Axes: slo:session, throughput
- Scenario: A client's network stalls; generated frames pile up faster than they're sent.
- System must: Use a bounded drop-oldest (or sender-side credit, G4/H2) egress buffer; stale audio nobody will hear is worthless — back-pressure the producer or shed the oldest.
- If mishandled: An unbounded high-water-mark (vLLM HWM=0) silently accumulates GBs of stale audio → OOM, then a hard crash.

### SLO-7 — Disable WS write-coalescing on the streaming route
- Level: Simple
- Pipeline: any streaming egress
- Axes: slo:session, qoe:jitter, priority:realtime
- Scenario: Frames are emitted every 80 ms but the socket layer batches writes by default.
- System must: Set per-frame flush (`write_buffer_size(0)`, F10) so each frame leaves immediately; coalescing adds tens-of-ms jitter to an 80 ms budget.
- If mishandled: Default Nagle/coalescing injects 10s-of-ms jitter, eroding the frame-deadline margin for no throughput gain.

### SLO-8 — Reject at admission, never admit-then-glitch
- Level: Simple
- Pipeline: any
- Axes: slo:session, priority:realtime, frame-deadline
- Scenario: A new realtime stream requests admission when every slot's frame budget is already committed.
- System must: Return a typed 429/503 + Retry-After at admission (§6, P-4) rather than accepting and stealing budget from live streams.
- If mishandled: Admit-and-degrade breaks the frame cadence for ALL live sessions instead of cleanly refusing one new one.

### SLO-9 — Cap per-session bookkeeping so a long-lived server doesn't leak
- Level: Simple
- Pipeline: any
- Axes: slo:session, throughput
- Scenario: A gateway runs for weeks accumulating per-stream counters, closed-sets, and chunk maps.
- System must: Cap every per-request map/set (trim 10000→5000, G6) and purge per-slot state on slot-free.
- If mishandled: Unbounded bookkeeping leaks until p99 latency climbs and the process eventually OOMs.

### SLO-10 — /readyz gates on warmup+calibration, not process-up
- Level: Simple
- Pipeline: any
- Axes: slo:TTFA, priority:realtime
- Scenario: The server process is alive but the CUDA-graph capture and per-stage calibration haven't finished.
- System must: Return non-200 from /readyz until warmup + calibration complete (C7, F6); only then admit traffic.
- If mishandled: The first real request pays seconds of capture/lazy-init (the first-request cliff) and blows its TTFA SLO.

### SLO-11 — Codec stage owns its own batch size (AR≥4, codec=1)
- Level: Simple
- Pipeline: 2-stage AR→codec TTS
- Axes: slo:session, throughput, priority:realtime
- Scenario: Two concurrent TTS streams; the AR stage pipelines fine but the codec window round-robins.
- System must: Give the codec stage its own `max_num_seqs` (typically 1) independent of the AR stage's ≥4 (C6, RFC #2568).
- If mishandled: A uniform batch-size default makes the codec window round-robin across requests → audible audio gaps under concurrency.

### SLO-12 — The bottleneck stage is the binding SLO, not the AR stage
- Level: Simple
- Pipeline: 3-stage AR→CFM→vocoder
- Axes: slo:e2e, frame-deadline, priority:realtime
- Scenario: AR decode is 9 ms/step but the CFM step at the same batch is 22 ms.
- System must: Decompose the session SLO into per-stage budgets and admit against the **bottleneck** stage's duty (§6), often the CFM/codec, not the cheap AR.
- If mishandled: Admitting on the AR stage's spare capacity oversubscribes the CFM stage → the slowest stage drops frames for everyone.

### SLO-13 — TTFA budget includes the acoustic delay, not just one step
- Level: Simple
- Pipeline: AR codec-TTS (Moshi-class)
- Axes: slo:TTFA, priority:realtime
- Scenario: A model has an acoustic delay of 2 frames before the first real audio token exists.
- System must: Budget first-audio = `frame_period + acoustic_delay·frame_period + step_time` (§4.4; Moshi ~160 ms theoretical, ~200 ms measured) and surface that as the rated TTFA.
- If mishandled: Promising a 1-step TTFA that ignores acoustic delay sets an SLO the model physically can't meet.

### SLO-14 — Low frame-rate is the single biggest realtime-throughput lever
- Level: Simple
- Pipeline: any AR codec
- Axes: throughput, slo:session, frame-deadline
- Scenario: Choosing between a 12.5 Hz (80 ms) codec and a 75 Hz (13.3 ms) codec for the same quality target.
- System must: Prefer the lower frame-rate where quality allows — 80 ms budget batches 16-32 streams @0.5B, while a 6.7 ms budget is sub-realtime even at batch 1 (§4.4, 12× spread).
- If mishandled: Picking a high-frame-rate codec collapses per-box concurrency and can make even a single stream miss its deadline.

### SLO-15 — Premium vs bulk: separate SLA tiers from the start
- Level: Simple
- Pipeline: any, mixed tenants
- Axes: priority:realtime, priority:batch, slo:session
- Scenario: A premium low-latency tenant and a bulk best-effort tenant share one box.
- System must: Classify into priority classes (Realtime > Batch, §6) so premium gets the slot/budget first and bulk fills leftover.
- If mishandled: FIFO mixing lets a bulk burst starve a premium caller, violating the paid SLA.

### SLO-16 — Barge-in cancels and reclaims the slot within ≤1 tick
- Level: Simple
- Pipeline: any conversational STS/TTS
- Axes: priority:realtime, slo:session, qoe:responsiveness
- Scenario: The user starts speaking while the assistant is mid-utterance.
- System must: Treat barge-in as a control message that jumps every stage's queue and frees the stream's slot/KV/window within ≤1 tick (§6, G2 reliable abort).
- If mishandled: A best-effort cancel (fire-and-forget PUB/SUB, G9) keeps generating audio over the user → the bot talks over them.

### SLO-17 — Cancelled must be distinguishable from completed
- Level: Simple
- Pipeline: any streaming egress
- Axes: slo:session, qoe:correctness, priority:realtime
- Scenario: A stream ends either because it finished its sentence or because the user barged in.
- System must: Emit an explicit FINAL frame for completion vs a distinct cancel terminal (G2); never infer "done" from absence of chunks.
- If mishandled: A consumer can't tell a finished turn from a stalled producer → premature close or an indefinite hang.

### SLO-18 — A reasoning-LLM turn needs a latency filler to mask TTFA
- Level: Simple
- Pipeline: cascaded STT→LLM→TTS, slow LLM
- Axes: slo:TTFA, slo:e2e, qoe:perceived-latency
- Scenario: A reasoning LLM takes 9-18 s to first token, far past any voice TTFA budget.
- System must: Fire a non-committal filler / backchannel immediately so perceived TTFA stays low while the slow tier runs in parallel (REALTIME_REASONING D2/D3).
- If mishandled: 9 s of dead air reads as a dropped call long before the real answer arrives.

### SLO-19 — Wall-clock aging: no call waits forever
- Level: Simple
- Pipeline: any, under load
- Axes: priority:realtime, fairness, slo:session
- Scenario: A low-priority stream keeps losing admission to a steady arrival of higher-priority streams.
- System must: Age the waiting request by wall-clock and promote after `max_wait` (H8; FCFS within the slot pool, hard per-slot fairness).
- If mishandled: vLLM-style priority with no aging term (#41951) starves the low-pri stream indefinitely — a dropped call.

### SLO-20 — Per-stream determinism only; don't promise bitwise cross-stream
- Level: Simple
- Pipeline: any AR
- Axes: slo:session, qoe:correctness
- Scenario: Two identical requests land in different batch positions and produce subtly different float reductions.
- System must: Guarantee per-stream determinism (seeded RNG per slot) but accept that batch-position atomic reductions make cross-stream bitwise identity impossible (H-other).
- If mishandled: Promising global determinism creates false bug reports and brittle tests over an unachievable property.

---

## INTERMEDIATE — competing objectives traded against each other

### SLO-21 — Latency vs aggregate-throughput: pick the batch knee, not max batch
- Level: Intermediate
- Pipeline: AR codec-TTS
- Axes: slo:session, throughput, frame-deadline
- Scenario: Batch 64 gives 55× throughput at 9.95 ms/step; batch 128 gives 84× but 13.08 ms/step.
- System must: Cap the lockstep batch at the largest N whose step still fits `0.8·T_f` (§4.4 `max_batch ≈ 0.8·T_f/t_step`), not the throughput-maximal N.
- If mishandled: Chasing 84× at batch 128 pushes step time toward the budget and starts dropping frames under any jitter.

### SLO-22 — Sarathi piggyback: fill leftover frame budget with Batch
- Level: Intermediate
- Pipeline: mixed realtime + batch
- Axes: priority:realtime, priority:batch, throughput
- Scenario: Realtime streams occupy 40 of the budget's worth of compute, leaving headroom each tick.
- System must: Piggyback Batch work into the leftover budget after Realtime is satisfied (Sarathi, §6) so aggregate throughput rises without touching realtime deadlines.
- If mishandled: Leaving the headroom idle wastes the batch/$ the box was bought for; over-filling it steals realtime budget.

### SLO-23 — EDF / least-deadline-first ordering across mixed sessions
- Level: Intermediate
- Pipeline: many streams, staggered deadlines
- Axes: priority:realtime, slo:session, frame-deadline
- Scenario: Several streams are due at different sub-budget offsets within the same tick window.
- System must: Order service by earliest deadline / risk-of-violation (VoxServe risk scheduling + Niyama EDF↔SRPF, L3/L9), serving the most-at-risk first.
- If mishandled: FIFO ordering serves a safe stream first and lets a near-deadline stream miss, even with total capacity to spare.

### SLO-24 — Risk-of-violation scheduling, not just raw deadline
- Level: Intermediate
- Pipeline: heterogeneous stages
- Axes: slo:session, frame-deadline, priority:realtime
- Scenario: Stream A is due sooner but its remaining work is tiny; stream B is due later but has a heavy CFM step ahead.
- System must: Prioritize by *risk* (deadline minus predicted remaining stage cost), not deadline alone (VoxServe, L3), so the genuinely-at-risk B isn't deprioritized.
- If mishandled: Naive least-deadline-first serves the safe-but-sooner A and lets the at-risk B blow its budget.

### SLO-25 — Graceful relegation under 50% overload (Niyama), not mass reject
- Level: Intermediate
- Pipeline: many streams, overloaded
- Axes: priority:realtime, slo:session, degradation
- Scenario: Offered load is 1.5× capacity for a sustained minute.
- System must: Relegate marginal streams to a degraded queue / lower service class (Niyama: 95%+ deadlines @50% overload vs <20% under blanket reject, L9) before any hard reject.
- If mishandled: Crude reject-don't-glitch drops far more sessions than necessary (80% violations vs 8.6%).

### SLO-26 — Quality-brownout before refusing (BrownoutServe)
- Level: Intermediate
- Pipeline: TTS with a quality dial (NFE / codebooks)
- Axes: degradation, slo:session, qoe:quality
- Scenario: Load exceeds capacity by ~10%; a small quality reduction would restore deadline compliance.
- System must: Brownout quality (e.g. fewer CFM steps / lower-precision tier) to absorb the overload (BrownoutServe: 74%→7% violations @ ~5% accuracy loss, L9) before rejecting calls.
- If mishandled: Refusing calls outright when a barely-noticeable quality drop would have kept everyone connected.

### SLO-27 — Prefill firewall: one voice-clone prefill must not stall N streams
- Level: Intermediate
- Pipeline: AR TTS, voice-clone
- Axes: priority:realtime, frame-deadline, slo:session
- Scenario: A long voice-clone reference prefill arrives while N synthesis streams are mid-utterance.
- System must: Admit ≤1 new prefill per K frames and chunk any prefill exceeding one frame-budget's worth of tokens (§4.5, Sarathi token budget keyed to the audio frame deadline).
- If mishandled: A naive prefill+decode hybrid inflates per-token TBT up to 28.3× → 17-22 dropped frames = total dropout for the live streams.

### SLO-28 — Firewall control variable is KV-length-aware predicted latency, not token count
- Level: Intermediate
- Pipeline: AR TTS, varying context
- Axes: frame-deadline, priority:realtime, slo:session
- Scenario: A token-budget of 8 produces >4× latency variation as the prefill's context grows.
- System must: Budget the firewall by **predicted latency** (KV-length / attention-aware, DuetServe/SlidingServe, L10), not a flat token count.
- If mishandled: A fixed token budget under-serves long-context prefills (8× infeasibility gap) and either stalls frames or wastes the GPU.

### SLO-29 — Keep prefill chunk sizes tile-aligned (power-of-two)
- Level: Intermediate
- Pipeline: AR prefill
- Axes: frame-deadline, slo:throughput
- Scenario: A chunk of 257 tokens is ~32% slower than 256 due to tile quantization.
- System must: Quantize chunk token counts to powers of two / GPU tiles (§4.5, Bullet wave-quantization L10) so the firewall's chunk fits a clean wave.
- If mishandled: An off-tile chunk silently costs ~32% extra, eating the frame margin the firewall was protecting.

### SLO-30 — Prefix-cache affinity: route returning-voice to the worker holding its ref-audio KV
- Level: Intermediate
- Pipeline: cloned-voice TTS, multi-worker
- Axes: throughput, slo:TTFA, priority:realtime
- Scenario: A user returns and re-requests their cloned voice; one worker already has that ref-audio KV resident.
- System must: Route the request to the worker holding the ref-audio prefix KV (L1: 86.4% avg / >90% peak hit, Fish-S2) so it skips re-prefilling the reference.
- If mishandled: Round-robin routing forfeits ~86% cacheable work, re-prefilling the reference and blowing TTFA on the top commercial workload.

### SLO-31 — Hybrid KV: radix prefix-cache for the ref/system prefix, ring for the suffix
- Level: Intermediate
- Pipeline: cloned-voice or system-prompted agent TTS
- Axes: throughput, slo:TTFA
- Scenario: Every request shares a deterministic ref-audio + system prompt then diverges per utterance.
- System must: Cache the deterministic prefix in a radix/paged cache and ring-buffer only the per-utterance suffix (L1 hybrid KV — the #1 fix).
- If mishandled: A pure per-slot ring recomputes the shared prefix every request, the single biggest avoidable cost under multi-tenant agents.

### SLO-32 — Prefix-cache key must fingerprint injected conditioning
- Level: Intermediate
- Pipeline: voice-clone TTS, shared cache
- Axes: qoe:correctness, slo:session
- Scenario: Two requests have identical placeholder token-ids but different ref-audio embeds pasted at `-100` positions.
- System must: Key the cache on a content hash over ALL codebooks of the injected conditioning (`extra_key=blake2b(...)`, G1/L1), with a `None` escape for genuinely-identical prefixes.
- If mishandled: RadixAttention concludes the prefixes match → cross-contaminates KV → silent wrong-voice output, only under concurrency.

### SLO-33 — Per-substrate duty ledger: NPU and GPU stages don't share compute
- Level: Intermediate
- Pipeline: heterogeneous placement (AR-GPU, codec-NPU)
- Axes: slo:throughput, frame-deadline, priority:realtime
- Scenario: Admission must check whether adding a stream fits both the GPU AR duty and the NPU codec duty.
- System must: Keep one compute-duty ledger per substrate and admit only if every substrate's `Σ duty ≤ S` (§6 per-substrate admission).
- If mishandled: Admitting on GPU headroom while the NPU codec is saturated drops frames at the codec stage the GPU ledger never saw.

### SLO-34 — Shared-bandwidth arbiter on unified memory (GB10 273 GB/s)
- Level: Intermediate
- Pipeline: heterogeneous DAG on GB10
- Axes: slo:throughput, frame-deadline
- Scenario: GPU AR decode and NPU codec both run, each demanding memory bandwidth from the one shared LPDDR ceiling.
- System must: Budget aggregate memory bandwidth as a schedulable resource (`Σ bandwidth_duty ≤ S·ceiling`, §3.4 contention guard) across all stages on the shared pool.
- If mishandled: Zero-copy removes transfer cost but concurrent engines divide the 273 GB/s ceiling → both stages slow and miss deadlines.

### SLO-35 — Cohort by (model, frame-rate); never lockstep-mix clocks
- Level: Intermediate
- Pipeline: multiple TTS models / frame-rates
- Axes: throughput, frame-deadline, priority:realtime
- Scenario: A 12.5 Hz stream and a 75 Hz stream both want service on the same AR thread.
- System must: Batch only within a `(model, frame_rate)` cohort (§4.2) and time-share cohorts via the duty ledger, never inside one fused step.
- If mishandled: Forcing two frame-rate clocks into one lockstep tick has no common realtime cadence → one or both underrun.

### SLO-36 — Head-of-line: chunk a long prefill so it doesn't block the queue
- Level: Intermediate
- Pipeline: AR TTS, mixed prefill+decode
- Axes: slo:TTFA, frame-deadline, priority:realtime
- Scenario: A long audio-prompt encode sits at the head of the stage queue ahead of many short decodes.
- System must: Force-chunk the long encode (`long_prefill_token_threshold`, H-other / #37308 147× TTFT HoL) so decodes interleave each tick.
- If mishandled: The long prefill head-of-line-blocks every short decode behind it → a 147× TTFT spike for all of them.

### SLO-37 — p99 ITL / jitter control, not just mean ITL
- Level: Intermediate
- Pipeline: AR, batched
- Axes: qoe:jitter, slo:session
- Scenario: Mean inter-token latency is fine but p99 spikes whenever a watermark-triggered eviction fires.
- System must: Tune for p99 ITL (non-zero watermark from fixed slots, H3: 0.05 → 187 preempts / 17.7 ms vs 0 → 1065 / 40 ms) and avoid allocation jitter (fixed ring KV, §4.3).
- If mishandled: A clean mean hides p99 spikes that the playback buffer can't absorb → periodic audible hitches.

### SLO-38 — Allocation jitter = frame-deadline misses → fixed ring KV
- Level: Intermediate
- Pipeline: AR codec-TTS
- Axes: frame-deadline, qoe:jitter, slo:session
- Scenario: A paged-KV allocation occasionally stalls a step mid-utterance.
- System must: Use a fixed per-slot ring/arena (§4.3) — zero reservation waste, no per-step block-table gather, no allocation jitter.
- If mishandled: Per-step paging jitter intermittently overruns the frame period even when average step time is fine.

### SLO-39 — Drift response ladder: stop admitting → shed Batch → shed newest Realtime ≤1/tick
- Level: Intermediate
- Pipeline: many streams, sustained p99 breach
- Axes: degradation, priority:realtime, priority:batch
- Scenario: The bottleneck stage's p99 breaches budget for a sustained window.
- System must: Apply the ordered ladder (FR-S3b, §6): stop admitting → shed Batch → only then shed the newest Realtime ≤1/tick with 60 s hysteresis.
- If mishandled: Shedding Realtime first (or shedding many at once) drops live calls that a Batch pause would have saved.

### SLO-40 — Shed newest, not oldest (preserve in-progress turns)
- Level: Intermediate
- Pipeline: overloaded realtime
- Axes: degradation, fairness, priority:realtime
- Scenario: One Realtime stream must be shed to restore cadence.
- System must: Shed the newest/least-progressed stream (§6) so half-finished utterances are preserved and the dropped one had the least sunk cost.
- If mishandled: Dropping an in-progress call mid-word is a far worse QoE event than refusing the freshest one.

### SLO-41 — Per-inference deadline is device+model-aware, not a flat 300 s
- Level: Intermediate
- Pipeline: any, mixed models/substrates
- Axes: slo:session, frame-deadline
- Scenario: A 1.5B AR-TTS step on GPU vs a CTC STT step on CPU have wildly different per-step costs.
- System must: Set the watchdog/inference deadline per device+model (H9: CPU needs 3600 s not 300 s; a 1.5B AR step ≠ a CTC step), not a single flat timeout.
- If mishandled: A flat 300 s deadline either kills slow-CPU work prematurely or lets a wedged GPU loop run far too long.

### SLO-42 — Progress watchdog keyed on last-audio-emitted
- Level: Intermediate
- Pipeline: any active streaming session
- Axes: slo:session, qoe:liveness
- Scenario: A session's loop is "alive" (passing health checks) but has emitted no audio for many frame intervals.
- System must: Track monotonic last-audio-emitted-at per session, checked by an independent thread; no audio for >N×frame-interval → kill/restart (H9, the #39863 blind spot).
- If mishandled: A zero-forward-progress loop passes every health check while the user hears dead air.

### SLO-43 — used/total slots gauge drives autoscale before saturation
- Level: Intermediate
- Pipeline: fleet, autoscaled
- Axes: throughput, slo:session
- Scenario: A box approaches its slot ceiling as call volume ramps.
- System must: Export used/total_slots and open-slots gauges (F9, §9 item 8) as the autoscale signal so capacity is added before reject kicks in.
- If mishandled: Discovering saturation only via 429s means callers already got rejected before scale-up triggered.

### SLO-44 — Warm over-provisioning; never scale-to-zero a voice tier
- Level: Intermediate
- Pipeline: fleet, bursty
- Axes: slo:TTFA, throughput, degradation
- Scenario: Traffic is bursty with idle troughs that tempt scale-to-zero.
- System must: Keep warm capacity / repurpose warm slots (L9: bursts 2.3 s TokenScale, cold 1.7-12.8 s BLITZSCALE) — never cold-start into a burst.
- If mishandled: A cold start adds 1.7-12.8 s to the first calls of a burst, blowing every TTFA SLO at the worst moment.

### SLO-45 — Mixed interactive + batch tenants on one box
- Level: Intermediate
- Pipeline: realtime calls + bulk transcription
- Axes: priority:realtime, priority:batch, throughput
- Scenario: Live conversational streams share a box with a large offline transcription backlog.
- System must: Run Batch strictly in Realtime's leftover budget (Sarathi piggyback, §6); Batch never preempts a frame deadline but soaks all idle capacity.
- If mishandled: Letting batch contend equally injects ITL jitter into live calls; isolating it entirely wastes the idle budget.

### SLO-46 — STT cache-aware streaming encoder for high concurrency
- Level: Intermediate
- Pipeline: streaming STT (FastConformer/RNN-T)
- Axes: throughput, slo:session, priority:realtime
- Scenario: Many concurrent live STT streams on one box.
- System must: Use a cache-aware streaming encoder emitting deltas with bounded state (Nemotron: 560 streams/H100, 3× baseline, L11) and lockstep-batch the chunked encoder.
- If mishandled: Re-encoding overlapping context per chunk caps concurrency at ~1/3 and wastes bandwidth.

### SLO-47 — Token-AR STT (Whisper-class) belongs on the paged path, not lockstep
- Level: Intermediate
- Pipeline: Whisper/Voxtral AED STT
- Axes: throughput, slo:session
- Scenario: Transcript token count varies per utterance and doesn't equal frame count.
- System must: Route token-AR STT to a continuous-batch + paged-KV admit/evict path (§4.1 matrix), not the fixed-slot frame-sync lockstep.
- If mishandled: Forcing variable-length transcript decode into a fixed-slot ring wastes slots and mis-paces against the frame clock.

### SLO-48 — Step-bucket the CFM/diffusion stage by (model, shape, NFE, CFG)
- Level: Intermediate
- Pipeline: CFM/flow TTS
- Axes: slo:throughput, frame-deadline
- Scenario: Several flow-decode requests with the same latent shape and step schedule arrive within ~2 ms.
- System must: Coalesce them into one step-bucket batch keyed `(model, latent-shape, step-schedule, CFG)` with a ~2 ms drain deadline (§4.2 step-bucket, G-other 2 ms vocoder micro-batch).
- If mishandled: Running each flow decode bs=1 wastes the compute-bound DiT's batch efficiency (10×@64 left on the table).

### SLO-49 — Streaming & non-streaming requests never share a batch
- Level: Intermediate
- Pipeline: mixed streaming + offline TTS
- Axes: slo:session, throughput
- Scenario: A streaming live request and a non-streaming offline request both want the same micro-batch.
- System must: Keep streaming and non-streaming in separate batches (G-other) — their cadence/latency contracts are incompatible.
- If mishandled: Mixing them either jitters the streaming session or starves the offline one of throughput.

### SLO-50 — Chunked-prefill mixed batch spikes TBT >8× → consider intra-node spatial P/D
- Level: Intermediate
- Pipeline: AR decode + prefill, GB10
- Axes: frame-deadline, qoe:jitter, priority:realtime
- Scenario: Chunked-prefill mixed into the decode batch makes a step 250 ms vs 15 ms decode-only.
- System must: A/B intra-node spatial prefill/decode partitioning (Nexus 20× TTFT / 2.5× TBT, TaiChi +77% goodput for strict-TPOT/relaxed-TTFT, L4) against the chunked-prefill firewall on GB10.
- If mishandled: Chunked-prefill injects an 8× TBT tail spike that intra-node SM-partitioning would have avoided.

### SLO-51 — MTP on the acoustic path: throughput without breaking lockstep
- Level: Intermediate
- Pipeline: AR codec-TTS
- Axes: throughput, slo:session
- Scenario: A model offers multi-token prediction (Depformer / MTP heads) emitting 3 tokens/step.
- System must: Use MTP as a direct-emit speedup (2-5× quality-neutral, VocalNet/FlashTTS, L14) that preserves the rectangular lockstep — explicitly NOT EAGLE/Medusa draft-spec-decode.
- If mishandled: Bolting draft-spec-decode on TTS destroys the rectangular batch and nets a 0.98× slowdown on acoustic tokens (L13/PCG).

### SLO-52 — Reliable abort channel with per-stage ack for barge-in
- Level: Intermediate
- Pipeline: multi-stage STS, barge-in
- Axes: priority:realtime, qoe:responsiveness
- Scenario: A barge-in must cancel an in-flight request spanning AR, CFM, and vocoder stages.
- System must: Use a reliable per-stage-acked abort (G9), fail-fast across all stages on one terminal signal; never fire-and-forget PUB/SUB.
- If mishandled: ZMQ-PUB-style abort drops to a not-yet-connected stage → that stage keeps synthesizing canceled audio.

### SLO-53 — NaN logit rejects the frame, never glitches it
- Level: Intermediate
- Pipeline: AR codec-TTS
- Axes: qoe:correctness, frame-deadline, priority:realtime
- Scenario: A logit row goes NaN; an argmax sampler would pick a garbage codec token.
- System must: Run an always-on `logits.isnan().any()` reduction and reject the frame (repeat prev / codec-silence / greedy-resample) (H1, the top inversion).
- If mishandled: An argmax over NaN emits an audible pop with zero error signal — silent QoE damage.

### SLO-54 — Sample outside the CUDA-graph (or graph-safe gumbel-argmax)
- Level: Intermediate
- Pipeline: AR codec-TTS, CUDA-graphed step
- Axes: slo:session, qoe:correctness
- Scenario: The lockstep step is CUDA-graphed for the edge latency win but TTS needs multinomial sampling.
- System must: Sample outside the captured region or use a graph-safe gumbel-argmax (C2/F7), keeping the deterministic step graphed.
- If mishandled: Capturing multinomial into the graph silently breaks sampling or forces eager, losing the 1.21×@B1 edge win.

### SLO-55 — Control-plane vs data-plane separation for jitter
- Level: Intermediate
- Pipeline: any streaming
- Axes: qoe:jitter, slo:session
- Scenario: Small control messages (admission, barge-in) and large raw PCM frames share one path.
- System must: Separate control-plane (small msgpack) from data-plane (raw PCM, zero-copy, ref-held until send done) (H-other).
- If mishandled: A large PCM write can delay an urgent barge-in control message → laggy cancellation and ITL jitter.

---

## COMPOUND — multiple interacting objectives + degradation

### SLO-56 — Premium realtime + bulk batch + a voice-clone prefill, all at once
- Level: Compound
- Pipeline: AR TTS, mixed tenants
- Axes: priority:realtime, priority:batch, slo:TTFA, frame-deadline
- Scenario: Premium live streams run at ~70% budget, a bulk transcription backlog waits, and a long voice-clone prefill arrives.
- System must: Hold premium frame deadlines first, chunk the clone prefill into ≤1-frame slices (§4.5), and piggyback Batch only in the slices' leftover (§6); the prefill never preempts a live frame.
- If mishandled: The prefill stalls premium frames (28.3× TBT), or batch contention jitters them, or the prefill starves waiting behind batch.

### SLO-57 — Returning-voice routing while the holding worker is near-saturated
- Level: Compound
- Pipeline: cloned-voice TTS, multi-worker
- Axes: throughput, slo:TTFA, priority:realtime, fairness
- Scenario: The worker holding a returning user's ref-audio KV (86% hit) is also the most loaded.
- System must: Weigh prefix-affinity (skip re-prefill, L1) against the holding worker's duty; if admitting there breaks its budget, either relegate the request or re-prefill on a freer worker rather than glitch the held streams.
- If mishandled: Blindly honoring affinity oversubscribes the hot worker; blindly load-balancing forfeits the 86% cache hit and TTFA.

### SLO-58 — Bursty arrival: aging vs deadline-ordering tension
- Level: Compound
- Pipeline: many streams, bursty
- Axes: fairness, priority:realtime, slo:session
- Scenario: A burst of high-priority streams keeps arriving while an aged low-pri stream's promotion comes due.
- System must: Let the aging term promote the starved stream (H8) even as EDF orders the rest by deadline (L9) — fairness floor under the deadline policy, both in one comparison key (the #41951 fix).
- If mishandled: Pure deadline-ordering starves the aged stream; pure aging ignores deadlines and misses fresh-but-urgent ones.

### SLO-59 — Cascaded STT→LLM→TTS: budget the whole mouth-to-ear chain
- Level: Compound
- Pipeline: 3-service cascade
- Axes: slo:e2e, slo:TTFA, priority:realtime
- Scenario: A turn must feel responsive end-to-end across three networked services with their own queues.
- System must: Allocate sub-budgets per stage, stream partial LLM output into TTS sentence-by-sentence (REALTIME_REASONING D2), and start TTS on the first sentence rather than the full LLM completion.
- If mishandled: Waiting for the full LLM response before TTS serializes three latencies into one long mouth-to-ear gap.

### SLO-60 — Reasoning-LLM agent + realtime streams sharing the box
- Level: Compound
- Pipeline: STS + reasoning-LLM tier
- Axes: slo:e2e, priority:realtime, throughput
- Scenario: A slow reasoning agent (9-18 s TTFT) runs alongside latency-critical voice streams on shared GPU.
- System must: Fire the latency filler immediately (D3), run the reasoning tier as lower-priority background work in leftover budget, and make barge-in cancel the LLM (REALTIME_REASONING invariant: barge-in-cancels-LLM).
- If mishandled: The reasoning tier's bursts jitter live streams, or an un-cancelled LLM keeps computing after the user moved on.

### SLO-61 — Bottleneck migrates from AR to codec under concurrency
- Level: Compound
- Pipeline: 2-stage AR→codec, scaling up
- Axes: frame-deadline, throughput, priority:realtime
- Scenario: At low concurrency AR is the cost; as streams grow, the codec window becomes the binding stage.
- System must: Track per-stage duty and re-target admission at whichever stage is currently binding (§6); the codec stage may cap concurrency well before the AR stage's ledger does.
- If mishandled: Admitting on stale "AR is the bottleneck" assumptions oversubscribes the codec → gaps appear only under load.

### SLO-62 — Heterogeneous placement frees GPU but divides shared bandwidth
- Level: Compound
- Pipeline: AR-GPU + codec-NPU on GB10
- Axes: slo:throughput, frame-deadline
- Scenario: Moving codec to the NPU frees GPU compute for more AR streams (target ≥1.3×) but both now pull the shared 273 GB/s.
- System must: Admit against both the per-substrate compute ledgers AND the shared-bandwidth budget (§3.4/§6); prefer overlapping the memory-bound AR with the compute-bound codec, co-locate+time-share if both saturate bandwidth.
- If mishandled: Counting only the freed GPU compute oversubscribes the shared bus → both stages slow and the 1.3× win evaporates into deadline misses.

### SLO-63 — Drift detected on bottleneck stage triggers the shed ladder
- Level: Compound
- Pipeline: many streams, near-saturation
- Axes: degradation, priority:realtime, priority:batch
- Scenario: The codec stage's p99 breaches budget for a sustained window while AR looks fine.
- System must: Detect drift on the BOTTLENECK stage's p99 (§6), stop admitting, shed Batch, brownout quality (L9), and only shed newest Realtime ≤1/tick if still breached, with hysteresis.
- If mishandled: Watching only the AR stage misses the codec drift entirely → frames drop with no mitigation triggered.

### SLO-64 — Long-form session: ring KV silently lossy without an escape hatch
- Level: Compound
- Pipeline: long TTS / many-turn agent
- Axes: qoe:correctness, slo:session, throughput
- Scenario: A 10-minute session exceeds the ring KV's bounded context (30k+ tokens).
- System must: Pin attention-sink tokens + provide a paged/full-context escape hatch for long-form (L12: StreamingLLM sink, AudioKV); detect when context outgrows the ring and switch path.
- If mishandled: The ring wraparound silently forgets early context (sliding-window instability) → coherence degrades mid-session with no signal.

### SLO-65 — Variable stream lifetime (barge-in/EOS/silence) breaks fixed-residency
- Level: Compound
- Pipeline: full-duplex STS
- Axes: priority:realtime, slo:session, throughput
- Scenario: Streams enter/leave/idle asynchronously via barge-in, end-of-turn, and VAD silence.
- System must: Handle heterogeneous residency first-class — model PAD/EPAD/SILENCE states (L7, BayLing-Duplex) and free/admit slots per-tick without assuming fixed residency.
- If mishandled: Assuming "voice fixes per-request rate" → jittery turn-taking and leaked/wasted slots as streams churn.

### SLO-66 — Masked idle slots are NOT free under heterogeneous residency
- Level: Compound
- Pipeline: lockstep AR, churning streams
- Axes: slo:throughput, degradation
- Scenario: At low occupancy most lockstep slots are masked-idle but still read/written by the dense kernel.
- System must: Compact/repack active slots OR explicitly budget the masked-slot energy/bandwidth cost (L8: idle-lane energy ~48% of serving energy; padding 13%@BS1→40%@BS32).
- If mishandled: Institutionalizing slowest-stream-paces-all wastes ~40% of the batch on padding and burns idle-lane energy.

### SLO-67 — Reject a stream because the bottleneck won't fit, even with AR slots free
- Level: Compound
- Pipeline: 3-stage TTS, near-saturation
- Axes: frame-deadline, priority:realtime
- Scenario: A new stream has a free AR slot but the CFM stage's duty is already at S.
- System must: Reject on the bottleneck-stage test (§6) — `∀ stage: active < calibrated max` — not on AR-slot availability.
- If mishandled: Admitting on the free AR slot pushes CFM over budget → the new stream AND existing ones miss frames.

### SLO-68 — DC spill: migrate a stream between replicas without a glitch
- Level: Compound
- Pipeline: DC fleet, rebalancing
- Axes: throughput, slo:session, frame-deadline
- Scenario: A replica overloads and a live stream should move to a freer one.
- System must: Use constant-time append-only KV migration (Llumnix ~sub-ms-5ms for voice ctx, L16/§6) and mask the ≥1-frame migration gap with the playback buffer.
- If mishandled: A naive migration drops ≥1 frame unbuffered → an audible glitch mid-call, or a slow copy stalls the stream.

### SLO-69 — Quality brownout choice must pass the perceptual gate
- Level: Compound
- Pipeline: TTS, overload brownout
- Axes: degradation, qoe:quality, slo:session
- Scenario: Under overload the system drops CFM NFE from 10 to 4 to save compute.
- System must: Verify the brownout tier against a perceptual/MOS gate (§5.2, not WER-only) before serving it (the WER-flat/MOS-crash AR-drift trap).
- If mishandled: A text-only gate passes a brownout that's measurably worse audio — saving compute by shipping degraded voice users notice.

### SLO-70 — Per-tenant cache salt: no latency side-channel from shared prefix cache
- Level: Compound
- Pipeline: multi-tenant, shared prefix cache
- Axes: qoe:correctness, slo:session, security
- Scenario: Two tenants could probe cache-hit latency to infer each other's cached prefixes.
- System must: Salt the block-0 cache key per tenant (H-other `cache_salt`) and use sha256 (never xxhash) so a collision can't leak cross-tenant KV.
- If mishandled: A shared unsalted cache leaks prefix presence via timing and risks a hash-collision cross-tenant KV leak.

### SLO-71 — Crash blast-radius: hot AR and flaky encoder as separate processes
- Level: Compound
- Pipeline: multi-stage, fault isolation
- Axes: slo:session, priority:realtime
- Scenario: A flaky encoder stage occasionally faults; it shares a process with the hot AR stage.
- System must: Place the hot AR and the flaky encoder in separate processes (G7) so an encoder crash fails its own requests, not every live AR stream; 3-layer crash detection fans the failure into each request queue.
- If mishandled: One co-located crash exits the whole process group → every live call drops simultaneously.

### SLO-72 — Dead sidecar = failed requests, not a hang
- Level: Compound
- Pipeline: torch-sidecar tier
- Axes: slo:session, priority:realtime
- Scenario: The Path-B torch sidecar dies mid-stream while the parent answers /health 200.
- System must: Use the death-sentinel byte + out-of-band waitpid/pidfd watcher → one `dead` flag that both rejects new admission and fans EngineDeadError into every live WS send within ~1 s (H6).
- If mishandled: The parent reports healthy while throughput is zero (#39863) → live sessions hang on dead air instead of fast-failing.

### SLO-73 — Slot leak on disconnect: multi-trigger free
- Level: Compound
- Pipeline: lockstep AR, live calls
- Axes: throughput, slo:session
- Scenario: A user disconnects but the disconnect callback is missed.
- System must: Free the slot from inside the step loop on ANY of {receiver closed, sender disconnected, send error, ping-timeout, idle-timeout} (F9), not solely on a callback; tail must finish draining first (F5).
- If mishandled: Leaked slots permanently shrink capacity until the box rejects new calls despite real idle headroom.

### SLO-74 — Tail still draining: don't free the slot on disconnect alone
- Level: Compound
- Pipeline: STT/TTS with acoustic delay
- Axes: qoe:correctness, slo:session
- Scenario: A stream's input ends but the model's delayed pipeline still owes the last words/frames.
- System must: Run the ACTIVE→MARKER_RECEIVED→IS_EOS lifecycle (F5); free the slot only after `offset ≥ real_end`, never on disconnect alone.
- If mishandled: Freeing on disconnect truncates the final words/audio of every turn.

### SLO-75 — Backpressure parks the upstream stage; admission tests the bottleneck
- Level: Compound
- Pipeline: multi-stage DAG
- Axes: frame-deadline, throughput, priority:realtime
- Scenario: A full downstream codec queue back-pressures the upstream AR stage.
- System must: Park (never drop) the upstream stage on a full bounded queue (§3.2) and ensure admission already tested the bottleneck so parking is rare.
- If mishandled: Parking the AR stage frequently because admission only checked AR capacity → stuttering across all streams.

### SLO-76 — Relay credit flow-control prevents fast-producer OOM
- Level: Compound
- Pipeline: sidecar↔stage relay
- Axes: throughput, slo:session
- Scenario: A fast AR producer outruns a slow codec consumer over the relay.
- System must: Use a bounded credit pool (default 2, G4) so the producer blocks until the consumer releases — natural throughput throttling without dropping or OOM.
- If mishandled: An unbounded relay overflows into OOM, or drops audio to keep up.

### SLO-77 — Determinism under brownout: seeded generator per CFG fold
- Level: Compound
- Pipeline: CFM, CFG-parallel
- Axes: qoe:correctness, slo:session
- Scenario: CFG is folded into the batch (×2) and the brownout changes NFE between requests.
- System must: Pass a seeded `generator` to each scheduler step (B/diffusion, G-other) so CFG-parallel matches sequential and per-stream output stays deterministic across brownout changes.
- If mishandled: An unseeded CFG fold diverges from sequential → non-deterministic audio that breaks regression tests and per-stream reproducibility.

### SLO-78 — Conditional-branch DAG: dynamic fan-in or it deadlocks
- Level: Compound
- Pipeline: STT→translate→TTS or thinker→talker→vocoder
- Axes: slo:e2e, priority:realtime
- Scenario: A text-only request produces no audio-encoder output but a fixed fan-in waits for all three sources.
- System must: Compute expected sources per-request (`wait_for_fn`, G11) so a non-firing branch doesn't block the merge; constrain routing to the static topology.
- If mishandled: A fixed `wait_for=[a,b,c]` deadlocks whenever a request's branch won't fire → the whole turn hangs.

### SLO-79 — Out-of-order arrival: vocoder gets AR chunks before its payload
- Level: Compound
- Pipeline: parallel-path DAG
- Axes: slo:e2e, qoe:correctness
- Scenario: A vocoder stage receives AR stream-chunks before its own request payload (no cross-path ordering).
- System must: Make pre-payload streaming explicit opt-in with monotone `chunk_id` per (req,target); latch the codec contract from whichever (payload|chunk-meta) arrives first (G-other).
- If mishandled: Silent corruption from consuming chunks before the contract is known, or a hard-fail when it should have buffered.

### SLO-80 — Variable inner-NFE streams can't share one lockstep tick
- Level: Compound
- Pipeline: AR-outer + diffusion-inner (DiTAR/FlashTTS class)
- Axes: frame-deadline, throughput, priority:realtime
- Scenario: Two streams run the same outer AR but request different inner NFE (10 vs 2) as a per-stream runtime dial.
- System must: Treat the inner solve as a per-stream variable-NFE micro-batch composed inside one AR step (L5 third class: compose two batchers per step), not a single shared inner schedule.
- If mishandled: Forcing both into one inner schedule either over-serves the NFE-2 stream or starves the NFE-10 one of steps.

### SLO-81 — Data-dependent frame-rate codec breaks fixed cohorting
- Level: Compound
- Pipeline: FlexiCodec-class TTS (3-12.5 Hz, data-dependent)
- Axes: frame-deadline, slo:throughput
- Scenario: A codec's frame-rate varies per-utterance and per-frame, unknown a priori.
- System must: Let lockstep advance a model-dependent **variable stride** and tolerate unknown-a-priori rates in the cohort key (L5/L6), re-deriving the step budget per stride.
- If mishandled: A fixed-rate cohort assumption mis-paces the variable-rate stream → underruns or wasted ticks.

### SLO-82 — Premium tenant SLA breach budget vs bulk best-effort
- Level: Compound
- Pipeline: multi-tier SaaS
- Axes: priority:realtime, priority:batch, slo:session, fairness
- Scenario: Honoring every premium frame would require shedding so much bulk that bulk SLA also breaches.
- System must: Protect premium's contracted deadline first, brownout/relegate bulk within its looser SLA (L9), and only shed bulk calls when bulk itself is at true saturation.
- If mishandled: Either premium breaches (lost revenue) or bulk is starved past its own SLA (lost revenue) — the tier policy must be explicit.

### SLO-83 — First-chunk-larger-then-smaller TTFA ramp under load
- Level: Compound
- Pipeline: streaming-vocoder
- Axes: slo:TTFA, qoe:quality, frame-deadline
- Scenario: The streaming vocoder can emit a larger first chunk for quality or a smaller one for faster TTFA.
- System must: Ramp the first chunk (larger for quality, then smaller for latency, §3.2 / I-other) and size it against current load so a heavy first chunk doesn't blow the frame budget under concurrency.
- If mishandled: A fixed large first chunk spikes TTFA under load; a fixed tiny one sacrifices first-audio quality needlessly when idle.

### SLO-84 — Async/overlap optimizations are net-negative at bs=1
- Level: Compound
- Pipeline: AR decode, single live call
- Axes: slo:session, throughput
- Scenario: A single live call (bs=1) is the hot path; one-step-lookahead async decode adds fixed event/bookkeeping overhead.
- System must: Gate pipelining/double-buffering on batch size (`async_decode_min_batch_size≥2`, G8; CUDA-graph-hurts-@batch-32 §1.3) and run synchronous at bs=1.
- If mishandled: Forcing the async path at bs=1 costs more than it saves and adds double-free landmines on abort/finish-during-overrun.

### SLO-85 — Calibrate per-stage max under synthetic co-load, keyed to warm-set
- Level: Compound
- Pipeline: any, admission calibration
- Axes: frame-deadline, slo:throughput
- Scenario: Admission needs the true `T_step(B_active)` per stage per substrate, which depends on co-resident load.
- System must: Calibrate `T_step` under synthetic co-load and persist keyed by `sha256 × device × driver × warm-set` (§6/§8.3b); the sidecar reports its footprint+duty at handshake.
- If mishandled: Admitting against an isolated single-stage benchmark overestimates capacity → frames drop once real co-load appears.

### SLO-86 — Measure calibration without the profiler attached
- Level: Compound
- Pipeline: calibration harness
- Axes: frame-deadline, slo:throughput
- Scenario: The calibration run is accidentally executed under a torch profiler.
- System must: Measure baseline latency WITHOUT the profiler and exclude the first (warmup) request (B/measurement-discipline); "Command Buffer Full" is profiler overhead, not a real target.
- If mishandled: Profiler-distorted numbers set the admission ceiling wrong → either under-utilization or live deadline misses.

### SLO-87 — Per-stage SLO decomposition with the bottleneck as binding
- Level: Compound
- Pipeline: 3-stage TTS
- Axes: slo:e2e, frame-deadline, priority:realtime
- Scenario: Session TTFA p90 ≤ budget and viability ≥99.9% must decompose into per-stage budgets.
- System must: Split into `T_step(stage,B) ≤ S·(1000/steps_per_second)` per stage with the bottleneck binding (§6); each stage carries its own SLO + duty entry.
- If mishandled: A single session-level SLO with no per-stage decomposition can't tell which stage to admit against or shed.

### SLO-88 — Eager fallback as a first-class OOM/capture escape
- Level: Compound
- Pipeline: AR, CUDA-graphed, low VRAM
- Axes: slo:session, frame-deadline
- Scenario: CUDA-graph capture OOMs on sm120 AFTER /health passed (#44209 crash-loop).
- System must: Make `enforce_eager` a first-class config + automatic downgrade-to-eager on capture failure (C8/H4 capability-driven ladder), never crash; pre-capture feasibility check at boot.
- If mishandled: A capture OOM crash-loops the box at request-1 instead of degrading to a slower-but-serving eager path.

### SLO-89 — Capture exact slot counts to sidestep the power-of-two cliff
- Level: Compound
- Pipeline: AR lockstep, CUDA-graph
- Axes: frame-deadline, slo:throughput
- Scenario: Padding the batch to the next power of two crosses the 257→272 / 257→512 tile cliff.
- System must: Capture graphs at the exact slot counts (1,2,4…N_slots) with zero padding (H4) so no step pays the padding cliff; `dst.zero_()` any padded slot.
- If mishandled: Padding to power-of-two adds wave-quantization cost per step or writes -1 into a real KV slot (#43810).

### SLO-90 — Two-phase outer loop: admissions/resets first, kernel only if any active
- Level: Compound
- Pipeline: lockstep scheduler
- Axes: throughput, frame-deadline, slo:session
- Scenario: A tick arrives when all slots are idle/masked.
- System must: Apply admissions/resets/control-plane FIRST, compute exec_mask, then run the kernel only if `exec_mask.any()` else short-sleep 1-2 ms (F6); never busy-spin, never run a kernel on an all-False batch.
- If mishandled: Busy-spinning the idle loop starves co-located stages (G3 ~600× slowdown) or a kernel over an all-masked batch corrupts state.

---

## EXTREME — near-saturation, 100-200 sessions, everything at once

### SLO-91 — 200 concurrent sessions: mixed premium-realtime + bulk-batch + reasoning-agent + an arriving clone prefill
- Level: Extreme
- Pipeline: full heterogeneous DAG, GB10 near-saturation
- Axes: priority:realtime, priority:batch, slo:TTFA, slo:e2e, frame-deadline, throughput
- Scenario: ~150 premium realtime streams, a bulk transcription backlog, one reasoning-LLM-backed agent, and a long voice-clone prefill all land near saturation.
- System must: Hold every realtime frame deadline (per-substrate + bottleneck + shared-bandwidth admission, §6), chunk the clone prefill into ≤1-frame slices behind a KV-length-aware firewall (L10), run the reasoning agent + bulk strictly in Sarathi leftover, fire the agent's latency filler, and drop NO call — relegate/brownout before any reject.
- If mishandled: The prefill or reasoning burst stalls premium frames → mass underrun across 150 calls, or blanket reject drops dozens of sessions that relegation would have saved.

### SLO-92 — Sustained 1.5× overload: maximize deadline compliance without dropping calls
- Level: Extreme
- Pipeline: many realtime streams
- Axes: degradation, priority:realtime, frame-deadline, slo:session
- Scenario: Offered realtime load holds at 1.5× box capacity for minutes.
- System must: Layer the full degradation stack — stop admitting, shed Batch, quality-brownout (NFE/precision), relegate marginal streams to a degraded class (Niyama 95%@50%-overload, L9), only shedding newest Realtime ≤1/tick as the last resort with hysteresis.
- If mishandled: Reject-only handling drops ~80% of sessions vs ~9% under graceful relegation — a catastrophic vs tolerable QoE gap.

### SLO-93 — Thundering herd: a burst of returning-voice requests all needing prefill
- Level: Extreme
- Pipeline: cloned-voice TTS fleet
- Axes: slo:TTFA, throughput, priority:realtime
- Scenario: A scheduled campaign fires 100 returning-voice calls in a second, each wanting its cached ref-audio.
- System must: Route by prefix-affinity to spread across workers holding each ref KV (L1 86% hit), firewall the genuine cache-misses' prefills (≤1/K frames each, §4.5), and warm-provision so no cold start adds 1.7-12.8 s (L9).
- If mishandled: All 100 prefills contend at once → either a prefill storm stalls live frames or affinity-blind routing forfeits 86% of the cache and TTFA collapses.

### SLO-94 — Cascade failure: a worker dies under 200-session load
- Level: Extreme
- Pipeline: DC fleet, replica fault
- Axes: slo:session, priority:realtime, throughput
- Scenario: One replica SIGKILLs while serving ~50 of 200 streams near saturation.
- System must: Fan the dead-flag into all its sessions in ~1 s (H6), migrate salvageable streams via append-only KV with playback-buffer-masked gaps (L16), and admission-throttle the rebalance so the surviving replicas don't cascade.
- If mishandled: The dead replica hangs its 50 sessions on dead air AND the rebalance stampede overloads survivors → fleet-wide cascade.

### SLO-95 — Simultaneous barge-ins across many active streams
- Level: Extreme
- Pipeline: full-duplex STS, high concurrency
- Axes: priority:realtime, qoe:responsiveness, throughput
- Scenario: A live event triggers dozens of users to interrupt their assistants within the same tick window.
- System must: Process each barge-in as a reliable per-stage-acked cancel reclaiming slot/KV/window within ≤1 tick (G9/§6), then re-admit the now-listening streams' new turns by deadline — all without a control-plane backlog (control/data separation, H-other).
- If mishandled: A flood of fire-and-forget aborts drops messages → many assistants keep talking over their users, and control-plane congestion delays the rest.

### SLO-96 — Near-saturation with a long-form many-turn agent in the mix
- Level: Extreme
- Pipeline: mixed short calls + a 20-minute agent session
- Axes: qoe:correctness, throughput, slo:session
- Scenario: 199 short streams plus one 20-minute many-turn agent whose context blows past the ring KV.
- System must: Move the long session to the paged/full-context escape with pinned attention-sinks (L12) while the 199 short streams stay on the efficient fixed-slot ring — admission accounts for the long session's larger paged footprint.
- If mishandled: Forcing the long session onto the ring silently forgets its early context, or its paged footprint un-budgeted starves the 199 short streams.

### SLO-97 — Frame-rate-diverse fleet: 12.5/25/75 Hz cohorts time-sharing one box
- Level: Extreme
- Pipeline: multi-model, multi-frame-rate
- Axes: throughput, frame-deadline, priority:realtime
- Scenario: 200 streams span three frame-rate cohorts that cannot lockstep-mix.
- System must: Batch each `(model, frame_rate)` cohort separately and time-share the GPU across cohorts via the duty ledger (§4.2/§6), budgeting each cohort's tick so no cohort's deadline is starved by another's duty.
- If mishandled: Co-scheduling cohorts without per-cohort duty budgeting lets the 75 Hz cohort's frequent ticks starve the 12.5 Hz cohort, or vice versa.

### SLO-98 — Prefill firewall under continuous clone-prefill arrival
- Level: Extreme
- Pipeline: AR TTS, steady clone onboarding
- Axes: frame-deadline, priority:realtime, throughput
- Scenario: New users continuously onboard cloned voices (each a long prefill) while 180 streams synthesize.
- System must: Admit ≤1 prefill per K frames with KV-length-aware predicted-latency chunking (L10), tile-align chunks (power-of-two, §4.5), and queue surplus prefills with aging so no onboarding starves — synthesis frames never miss.
- If mishandled: Unthrottled continuous prefills inflate TBT (up to 28.3×) across 180 streams → sustained dropout, or starved onboarding leaves new users waiting indefinitely.

### SLO-99 — Bandwidth saturation on GB10 with full heterogeneous placement
- Level: Extreme
- Pipeline: AR-GPU + CFM-GPU + codec-NPU + encoder-CPU, near-saturation
- Axes: slo:throughput, frame-deadline
- Scenario: All four stage types run across GPU/NPU/CPU and collectively saturate the 273 GB/s LPDDR ceiling.
- System must: Treat aggregate bandwidth as the binding budget (§3.4 contention guard / §6 shared ledger) — overlap memory-bound AR with compute-bound codec, co-locate+time-share the two bandwidth-saturating stages, and reject/relegate before the shared bus oversubscribes.
- If mishandled: Per-substrate compute ledgers all show headroom while the shared bus is saturated → every stage slows together and the whole box misses deadlines invisibly.

### SLO-100 — p99 ITL guarantee across 150 streams during a quality brownout
- Level: Extreme
- Pipeline: AR+CFM, overload brownout
- Axes: qoe:jitter, slo:session, degradation
- Scenario: A brownout drops CFM NFE to restore capacity while 150 streams demand stable p99 ITL.
- System must: Apply the brownout uniformly within a cohort (no per-stream NFE divergence on a shared lockstep tick, L5), verify it passes the MOS gate (§5.2), and keep the non-zero watermark to hold p99 ITL (H3) so the brownout reduces quality smoothly, not jitter.
- If mishandled: A ragged per-stream brownout desyncs the lockstep cohort and spikes p99 ITL → audible hitches layered on top of reduced quality.

### SLO-101 — 200-session admission storm: reject cleanly at true saturation, relegate before it
- Level: Extreme
- Pipeline: full DAG, admission boundary
- Axes: priority:realtime, degradation, slo:session, frame-deadline
- Scenario: Arrivals push the box from 95% to genuine 100%+ saturation within seconds.
- System must: Relegate/brownout up to true saturation, then issue typed 429/503 + Retry-After (§6, P-4) only for the streams that genuinely won't fit any stage's bottleneck-budget — never admit-and-degrade past the wall.
- If mishandled: Either premature rejects below saturation (lost capacity) or admit-past-saturation glitches every live stream (catastrophic).

### SLO-102 — Reasoning agent barge-in during 200-session peak cancels LLM and reclaims compute
- Level: Extreme
- Pipeline: STS + reasoning tier, peak load
- Axes: priority:realtime, throughput, slo:e2e
- Scenario: At peak the user barges in on the reasoning agent mid-think while 199 other streams run.
- System must: Cancel the in-flight LLM immediately (barge-in-cancels-LLM invariant), reclaim its leftover-budget compute for the realtime streams within ≤1 tick (§6), and re-admit the user's new turn by deadline — without poisoning any other session's barge-in state.
- If mishandled: An un-cancelled reasoning LLM keeps consuming leftover budget after the user moved on, denying the 199 realtime streams the compute they need to hold frames.

### SLO-103 — Calibration drift mid-shift: warm-set changes under 200-session load
- Level: Extreme
- Pipeline: any, long-running fleet
- Axes: frame-deadline, throughput, slo:session
- Scenario: A co-resident model loads/unloads mid-shift, invalidating the `T_step` calibration the admission ceiling relied on.
- System must: Re-key calibration on the new warm-set (§6/§8.3b), conservatively tighten admission until re-calibration completes, and never trust a stale ceiling that the changed co-load has invalidated.
- If mishandled: Admitting against a stale calibration after the warm-set changed overestimates capacity → frames start dropping with no apparent cause.

### SLO-104 — Variable-stride + variable-NFE + 200 streams: the third execution class at scale
- Level: Extreme
- Pipeline: AR-outer + generative-inner (FlashTTS/DiTAR class), high concurrency
- Axes: frame-deadline, throughput, priority:realtime
- Scenario: 200 streams of an MTP-3 + 2-NFE-meanflow model that breaks both batchers in one model, with per-stream NFE dials.
- System must: Run the third execution class (L5) — outer AR advances a variable stride, the inner solve is a per-stream variable-NFE micro-batch composed inside one step — and fold `T_step = T_ar + inner_steps×T_inner` into admission per-stream so the schedulable budget stays correct at scale.
- If mishandled: Treating it as a single fixed batcher mis-paces the variable-stride/variable-NFE streams → either underruns or wasted ticks across all 200.

### SLO-105 — Graceful end-to-end: 0% dropped calls is the headline acceptance bar
- Level: Extreme
- Pipeline: full system, sustained peak
- Axes: priority:realtime, degradation, fairness, slo:session
- Scenario: A sustained peak with mixed tenants where naive systems would drop calls.
- System must: Combine wall-clock aging (no starvation, H8), graceful relegation/brownout (L9), bottleneck-aware admission (§6), and warm provisioning (L9) so that under realistic peak NO realtime call is dropped — quality degrades before connectivity does.
- If mishandled: Any of the four missing → dropped/starved calls reappear; the system fails its primary QoE promise of never hanging up on a user.

---

## Coverage

This catalog covers **105 scenarios** across the request-prioritization / SLO / QoE / latency-vs-throughput family, structured SIMPLE → INTERMEDIATE → COMPOUND → EXTREME.

**Competing objectives & their trade-offs:** TTFA/first-audio as the felt latency (SLO-1, 13, 18, 83); RTF<1 per-session survival (SLO-2); end-to-end mouth-to-ear for cascades (SLO-59, 60); aggregate vs per-session throughput (SLO-4, 21, 22); the batch-knee (not max-batch) latency↔throughput trade (SLO-21); per-stage SLO decomposition with the bottleneck binding (SLO-12, 61, 67, 87).

**VoxServe binary streaming-viability (L3):** once-delivered-in-time-is-worthless and don't-over-serve (SLO-3); risk-of-violation scheduling (SLO-23, 24); playback-buffer cadence protection (SLO-5, 68, 94).

**Realtime > Batch + Sarathi piggyback (§6):** priority classes (SLO-15, 45), leftover-budget piggyback (SLO-22, 56, 91), reasoning-tier-as-background (SLO-60, 102).

**Deadline-aware scheduling:** EDF/least-deadline/risk (SLO-23, 24), free token pacing from frame-sync (SLO-4, §4.6).

**Graceful degradation as PRIMARY (L9):** Niyama relegation (SLO-25, 92), BrownoutServe quality-brownout (SLO-26, 69, 100), the drift→shed ladder (SLO-39, 40, 63), hard-reject only at true saturation (SLO-8, 101, 105).

**Prefill firewall (§4.5/L10):** one-clone-prefill-must-not-stall-N (SLO-27, 56, 91, 98), KV-length-aware predicted-latency control variable (SLO-28), tile-alignment (SLO-29), head-of-line avoidance (SLO-36), intra-node spatial P/D option (SLO-50).

**Prefix-cache affinity (L1/G1):** 86% returning-voice routing (SLO-30, 57, 93), hybrid radix+ring KV (SLO-31), conditioning-fingerprint keys (SLO-32), per-tenant salt (SLO-70).

**Fairness/anti-starvation (H8):** wall-clock aging / no dropped calls (SLO-19, 58, 105), shed-newest (SLO-40).

**Barge-in priority (§6/G9):** cancel-and-reclaim ≤1 tick (SLO-16, 52, 95), cancelled≠completed (SLO-17), barge-in-cancels-LLM (SLO-60, 102).

**Per-substrate duty ledger + shared-bandwidth arbiter (§3.4/§6):** NPU/GPU separation (SLO-33), GB10 273 GB/s contention (SLO-34, 62, 99), cohort-by-frame-rate (SLO-35, 97), variable-stride/data-dependent-rate (SLO-81, 104).

**Jitter / p99-ITL control:** non-zero watermark (SLO-37), allocation-jitter→ring-KV (SLO-38), WS coalescing (SLO-7), control/data separation (SLO-55), p99 under brownout (SLO-100).

**Mixed interactive+batch + SLA tiers:** (SLO-15, 45, 82); over-generation waste at ~3.3 tok/s (SLO-4).

**Liveness / fault QoE:** progress watchdog (SLO-42), dead-sidecar fast-fail (SLO-72), crash blast-radius isolation (SLO-71), slot-leak multi-trigger (SLO-73), tail-draining (SLO-74), worker-death cascade (SLO-94).

**Correctness invariants that are QoE:** NaN-rejects-frame (SLO-53), sample-outside-graph (SLO-54), eager fallback (SLO-88), exact-slot capture (SLO-89), two-phase loop (SLO-90), determinism under brownout (SLO-77).

**Calibration discipline:** synthetic-co-load keyed to warm-set (SLO-85), no-profiler measurement (SLO-86), mid-shift drift (SLO-103).

**EXTREME peak:** the full 200-session mix (SLO-91), 1.5× overload (SLO-92), thundering-herd prefill (SLO-93, 98), simultaneous barge-ins (SLO-95), long-form-in-the-mix (SLO-96), bandwidth saturation (SLO-99), 0%-dropped-calls headline (SLO-105).

**Deliberately scoped to this family:** model-accuracy/WER correctness, pure numerics/dtype parity, transport/codec encoding, and DAG-topology-construction details are referenced only where they directly drive an SLO/QoE outcome (e.g. MOS gate on brownout SLO-69/100, codec-dtype not covered) — the mechanics themselves belong to the model-onboarding, numerics, and DAG-construction catalogs.
