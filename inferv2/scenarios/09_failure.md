# 09 — Failure / Recovery / Production-Hardening Scenarios

Real situations where WaaV Infer breaks and how the engine must survive them. Each is a failure mode mined from proven systems (vLLM-core H1–H9, Moshi F1–F10, SGLang-Omni G1–G11, vLLM-Omni I1–I5) and re-grounded on the v2 architecture (frame-synchronous lockstep, per-slot ring KV, heterogeneous stage-DAG, CUDA-graph ladder on GB10/sm120, Path-B torch sidecar + relay). The governing law for the AR spine: **MASKED ≠ ABSENT** — an idle slot is still in the dense batch and its data must be made harmless, never skipped. The governing policy: **reject-don't-glitch at admission; never preempt mid-utterance; a dropped half-utterance is an audible failure.**

Levels: **SIMPLE** (one fault, one stage) → **INTERMEDIATE** (interaction across slots/stages) → **COMPOUND** (two faults compose) → **EXTREME** (cascading multi-subsystem failure).

---

## SIMPLE — single fault, single stage

### FAIL-1 — NaN logit argmaxed to a garbage codec token
- Level: SIMPLE
- Pipeline: AR codec-TTS / AR-STT decode
- Axes: numerics, fail:NaN, fail:corrupt
- Scenario: A logit row goes NaN (bad input, overflow, dead neuron). The V1-style sampler is multinomial-free (`probs.div(q).argmax`) so it cannot raise — it argmaxes the NaN row to an arbitrary codec token and emits an audible pop with zero error signal.
- System must: Run an always-on `logits.isnan().any()` (one cheap reduction) before sampling; on hit, REJECT THE FRAME — repeat previous frame / emit codec-silence / greedy-resample — never let the garbage token reach the codec. (vLLM H1 inversion.)
- If mishandled: User hears a sharp pop/click, no log, no metric, no recovery.

### FAIL-2 — +Inf logit → fp16 softmax overflow → whole row NaN
- Level: SIMPLE
- Pipeline: AR decode (fp16 path)
- Axes: numerics, fail:NaN
- Scenario: A long-context attention score in fp16 exceeds 65504 → +Inf → softmax produces NaN across the row, then propagates to every subsequent frame for that slot.
- System must: Prefer bf16 over fp16 for long-context attention by default; keep sampler/softmax-pivot math in fp32 regardless of model dtype; the FAIL-1 NaN guard catches the leak as a backstop. (vLLM H5.)
- If mishandled: One overflow silently corrupts the rest of the utterance.

### FAIL-3 — tiny-temperature exp() blow-up
- Level: SIMPLE
- Pipeline: AR sampling head
- Axes: numerics, fail:NaN
- Scenario: A caller passes `temperature=1e-9`; `logits/temp` explodes the exp() to Inf and the distribution becomes degenerate/NaN.
- System must: Clamp with `_MAX_TEMP` semantics (warn + clamp tiny temps) and fold `temp < _SAMPLING_EPS (1e-5)` to a greedy argmax path; never divide raw by an unclamped near-zero. (vLLM H5.)
- If mishandled: Sampler emits NaN/garbage tokens on a hostile or fat-fingered temperature.

### FAIL-4 — all-masked logits after top-p/top-k
- Level: SIMPLE
- Pipeline: AR sampling head
- Axes: numerics, fail:corrupt
- Scenario: Aggressive `top_p`/`top_k` (or a degenerate distribution) masks out every candidate → no survivor → sampler reads from an empty support.
- System must: Guarantee ≥1 survivor (`top_p_mask[:,-1] = False`) so the most-probable token always remains; use the NaN-safe `if not (pivot < max_logit)` idiom so a NaN pivot is caught too. (vLLM H5.)
- If mishandled: Index-out-of-range crash or a uniform-garbage sample.

### FAIL-5 — multinomial sampler captured inside a CUDA graph
- Level: SIMPLE
- Pipeline: AR lockstep step (graphed)
- Axes: CUDA-graph, fail:corrupt
- Scenario: TTS needs `torch.multinomial`, but multinomial is not CUDA-graph-safe; capturing it either errors at capture or silently replays a stale RNG state, repeating the same token sequence.
- System must: Sample OUTSIDE the captured region, or use a graph-safe gumbel-argmax inside it; the graphed callable covers only the deterministic forward. (vLLM-Omni I-CUDA-graph, resolves critique C2.)
- If mishandled: Every stream gets identical (or frozen) sampled audio under graph mode.

### FAIL-6 — codec decoder run under autocast → silent audio degradation
- Level: SIMPLE
- Pipeline: codec/vocoder stage
- Axes: numerics, quant, fail:corrupt
- Scenario: The Mimi/DAC/SNAC codec decoder inherits the AR stage's bf16/autocast context; codec math is precision-sensitive and the output is subtly noisy/metallic — WER stays flat, MOS crashes.
- System must: Force codec/vocoder + norms/RoPE/sampling-head to fp32 by architecture default; the codec stage never inherits the AR dtype. (vLLM-Omni phase-2 rule, §5.2.)
- If mishandled: Text-only (WER) gate passes; humans hear degraded audio in production.

### FAIL-7 — cumulative re-decode instead of delta streaming
- Level: SIMPLE
- Pipeline: streaming egress
- Axes: streaming, fail:corrupt
- Scenario: The streaming path yields cumulative audio (re-decoded from step 0) each chunk instead of only new samples; offline RTF passes but the listener hears replays/stutter, and cost is O(N²).
- System must: Emit delta-only (new samples since last chunk); enforce a byte-identical `offline_concat == stream_concat` invariant test; audit the consolidate path so it concats the audio key (never `continue`). (vLLM-Omni I1, resolves C1.)
- If mishandled: Audible replay/stutter and quadratic latency growth over a long utterance.

### FAIL-8 — per-step D2H sync collapses the frame budget
- Level: SIMPLE
- Pipeline: Path-B torch sidecar AR/CFM loop
- Axes: jitter, fail:hang
- Scenario: The sidecar's per-frame loop calls `.item()/.cpu()/.tolist()` (e.g. to check a stop condition) — "10 steps × 60 frames × 4 ops = 2400 syncs/request" — each a GPU→CPU stall, and the 9 ms step balloons past the frame budget.
- System must: Forbid all D2H syncs in any per-step loop; use `dst.copy_(src)` not `fill_(src.item())`, masking/`torch.where` not Python branches; a zero-D2H-sync test (CUDA-event/profiler guard) gates the sidecar. (vLLM-Omni I3, resolves C5.)
- If mishandled: Steady underruns/dropouts that only appear under real streaming, not offline benchmarks.

### FAIL-9 — `torch.cuda.empty_cache()` in the hot path
- Level: SIMPLE
- Pipeline: Path-B sidecar / any per-frame loop
- Axes: jitter, fail:hang
- Scenario: A per-call `empty_cache()` (added as an OOM band-aid) inserts a full device sync → a GPU idle bubble every frame → cadence jitter.
- System must: Never call `empty_cache()` in the per-frame loop; only on slot-free under measured memory pressure. (vLLM-Omni diffusion-perf rule.)
- If mishandled: Periodic frame-deadline misses (audible gaps) under sustained load.

### FAIL-10 — WS write-buffer coalescing adds jitter to an 80 ms budget
- Level: SIMPLE
- Pipeline: transport (WebSocket egress)
- Axes: jitter, transport
- Scenario: The default socket write buffer coalesces small frames, adding tens of ms of batching jitter to an 80 ms frame budget — first-audio and cadence both wobble.
- System must: Set `write_buffer_size(0)` (flush-per-frame) on every streaming route; treat per-stream buffer depth as a first-class metric. (Moshi F10.)
- If mishandled: Listener hears uneven cadence even though the engine met every step deadline.

### FAIL-11 — HWM=0 stale-audio pileup on a slow consumer
- Level: SIMPLE
- Pipeline: streaming egress buffer
- Axes: leak, fail:OOM
- Scenario: A slow TTS consumer (slow network) drains slower than the producer; with an unbounded high-water-mark, the per-stream queue silently accumulates GBs of stale, already-worthless audio.
- System must: Use a bounded queue with drop-oldest on the realtime audio path (stale frames have no value once the clock passes them); cap depth and meter drops. (vLLM H2.)
- If mishandled: Per-session memory grows unbounded → eventual OOM from one slow client.

### FAIL-12 — slot leak when a disconnect callback is missed
- Level: SIMPLE
- Pipeline: lockstep scheduler / WS session
- Axes: leak, fail:leak
- Scenario: A client vanishes (network drop) and the single disconnect callback is missed; the slot, KV, conv ring, and word buffers are never freed → permanent slot starvation.
- System must: Free the slot from INSIDE the step loop on ANY of {receiver closed, sender disconnected, send error, ping-timeout 20s, idle-timeout 120s} — layered triggers, never sole reliance on one callback; expose used/total_slots gauge. (Moshi F9.)
- If mishandled: Slots leak one at a time until the server admits nobody.

### FAIL-13 — never-busy-spin: idle scheduler loop starves co-located stages
- Level: SIMPLE
- Pipeline: scheduler outer loop
- Axes: jitter, fail:hang
- Scenario: The lockstep loop is `loop { try_recv() }` with no idle yield; when no slot is active it pins a CPU core (GIL-equivalent starvation in Python; runtime/core starvation in Rust), slowing a co-located encoder/codec stage ~600×.
- System must: Block on `recv_timeout`/`Notify` when idle (short 1–2 ms sleep), hog the core only when `exec_mask.any()`; run the kernel only on a non-empty batch. (SGLang G3 / Moshi F6.)
- If mishandled: Co-located stages crawl whenever the AR loop idles between turns.

### FAIL-14 — kernel run on an all-False (all-idle) batch
- Level: SIMPLE
- Pipeline: lockstep scheduler
- Axes: corrupt, fail:corrupt
- Scenario: Every slot is idle for a tick but the loop still launches the dense step kernel over an all-masked batch — wasted launch, and the substituted-token path may be untested for the empty case.
- System must: Apply admissions/resets/control-plane first, compute `exec_mask`, then run the kernel only if `exec_mask.any()` else short-sleep. (Moshi F6.)
- If mishandled: Wasted GPU launches and a latent crash path when the batch is fully idle.

### FAIL-15 — readiness reports OK before warmup/calibration completes
- Level: SIMPLE
- Pipeline: lifecycle / health
- Axes: hang, corrupt
- Scenario: `/readyz` returns 200 as soon as the process is up; the load balancer routes a stream before CUDA-graph capture and calibration finish, so request-1 pays seconds of capture on the hot path (or hits an uncalibrated admission ledger).
- System must: Gate `/readyz` on warmup (2–3 full-mask steps + `synchronize()`) AND calibration complete, not process-up; warmup forces graph capture off the hot path. (Moshi F6 / vLLM-Omni E2E rule, resolves C7.)
- If mishandled: First user after every (re)deploy gets a multi-second stall or a mis-admission.

### FAIL-16 — hung forward never returns (flat 300s timeout)
- Level: SIMPLE
- Pipeline: any inference stage
- Axes: hang, fail:hang
- Scenario: A forward wedges (driver bug, deadlock) and a single flat 300 s timeout is far too long for a voice step but too short for a CPU batch path — either way the stream hangs.
- System must: Use a device + model-aware per-inference deadline (1–5 s for a GPU AR/codec step; longer for a CPU/edge path), not a flat 300 s; on breach, kill/restart the stage. (vLLM H9, #45135.)
- If mishandled: A wedged step silently freezes the session for minutes.

### FAIL-17 — poison-pill request crashes the worker
- Level: SIMPLE
- Pipeline: ingress / model forward
- Axes: crash, fail:crash
- Scenario: A malformed request (NaN audio samples, absurd length field, unsupported codebook count) triggers a shape-mismatch or assertion deep in the forward and takes down the worker.
- System must: Validate at ingress (shape/dtype/duration/codebook asserts on presence, never tensor-truthiness); reject the one request with a typed 4xx; the forward never sees an out-of-contract payload. (vLLM-Omni I2.)
- If mishandled: One crafted request kills every co-resident session in the process.

### FAIL-18 — quant accuracy divergence on hard audio (WER-flat / MOS-crash)
- Level: SIMPLE
- Pipeline: quantized AR-TTS / STT
- Axes: quant, contaminate, fail:corrupt
- Scenario: An int8/fp8 variant passes a text-only WER gate but drifts on hard audio (accent, noise, long utterance) — the classic AR-drift signature where quant noise compounds across frames.
- System must: Gate quant variants with a perceptual/MOS check + streaming + concurrent layers (not WER alone) vs `reference_precision`; persist a `verified{substrate,precision,metric}` stamp; unverified ⇒ refuse or fall back to reference precision + emit `waav_quant_gate_failed`. (vLLM-Omni I4, §5.2, resolves C4.)
- If mishandled: A variant that "passed" ships audibly degraded audio on real-world inputs.

### FAIL-19 — int8 weights land on ORT-CUDA → 20× latency LOSS
- Level: SIMPLE
- Pipeline: load-time precision resolution
- Axes: quant, fail:hang
- Scenario: An int8/4-bit checkpoint is selected for the CUDA EP, but ORT-CUDA can't run int8 GEMM (`MatMulInteger`/Q-DQ silently partition to the CPU EP → measured 12 ms → 232 ms); the "memory win" becomes a latency catastrophe.
- System must: Resolve precision per active EP (`$WAAV_PRECISION → by_substrate[ep] → precision → fp32`) so an int8 file never lands on ORT-CUDA; route GPU quant to the TensorRT EP (S8S8) or the torch sidecar tier. (§5.2 master constraint.)
- If mishandled: A "faster" quantized model is ~20× slower and silently sub-realtime.

### FAIL-20 — clock overrun: one step exceeds the frame budget
- Level: SIMPLE
- Pipeline: AR or CFM stage
- Axes: jitter, fail:hang
- Scenario: A step momentarily overruns the frame period (thermal blip, a 10-step CFM solve at B64 = 110 ms over a 40 ms budget) → an audible underrun if unmanaged.
- System must: Have an EXPLICIT overrun policy — meter per-step wall time vs budget; absorb a transient via the client playback buffer; sustained overrun trips drift response (stop admitting), never silently drop frames for everyone. (Moshi F10 / §6.)
- If mishandled: Random underruns with no policy and no signal.

### FAIL-21 — marker-drop truncates the transcript at clip end
- Level: SIMPLE
- Pipeline: STT streaming / one-shot-over-streaming-core
- Axes: streaming, corrupt
- Scenario: The "stream done" marker fires at input-exhaustion instead of `now + asr_delay + buffered_frames`, so the delayed model never emits its last words — the transcript is truncated.
- System must: Schedule the marker in a step-ordered `BinaryHeap` at the future flush step; for non-streaming input, append the real audio + marker + 10 s trailing silence and terminate on the marker, not input-exhaustion. (Moshi F5.)
- If mishandled: Every transcript silently loses its final word(s).

### FAIL-22 — missing FINAL frame: consumer can't tell done from stalled
- Level: SIMPLE
- Pipeline: streaming egress contract
- Axes: streaming, hang
- Scenario: The consumer infers "done" from absence-of-chunks; a stalled producer looks identical to completion → premature close or indefinite hang.
- System must: Send an EXPLICIT FINAL/`is_done` sentinel frame; "closed without FINAL" is the failure signal; never infer end-of-stream from silence. (SGLang G2.)
- If mishandled: Clients either cut off early or wait forever on a dead producer.

### FAIL-23 — GPU thermal throttle silently inflates step time
- Level: SIMPLE
- Pipeline: any GPU stage
- Axes: jitter, fail:hang
- Scenario: Sustained load heats the GB10; the clock throttles and the calibrated 9 ms step quietly becomes 14 ms, pushing a previously-safe batch over the frame budget.
- System must: Treat measured step time (not the calibration constant) as the live admission input; a sustained p99 breach on the bottleneck stage trips drift response (stop admitting → shed Batch → shed newest Realtime ≤1/tick) with hysteresis. (§6 drift response.)
- If mishandled: A box that passed calibration starts dropping frames as it warms up, with no adaptation.

### FAIL-24 — control-plane / data-plane not separated → a PCM blob blocks a control message
- Level: SIMPLE
- Pipeline: IPC / relay
- Axes: jitter, fail:hang
- Scenario: Small control messages (admit, cancel, marker) share one channel with raw-PCM data frames; a large audio payload in flight delays a time-critical cancel/marker behind it.
- System must: Separate the control plane (small msgpack, its own channel) from the data plane (raw PCM, zero-copy, ref-held until send completes); control never queues behind data. (vLLM H-other.)
- If mishandled: Barge-in/marker latency spikes whenever a big audio frame is mid-transfer.

### FAIL-25 — `max_num_batched_tokens` below `num_slots` → startup mis-sized batch
- Level: SIMPLE
- Pipeline: scheduler config / boot
- Axes: corrupt, fail:hang
- Scenario: The fixed per-frame token budget is configured smaller than the slot count, so the rectangular batch can't even hold one frame per active slot.
- System must: Compute `max_num_batched_tokens = num_slots × per-frame-cost` (fixed+small) and enforce `≥ num_slots` with a startup crash if violated — fail at boot, never silently truncate the batch. (vLLM H-other.)
- If mishandled: The batch silently drops slots, or a confusing mid-run shape error appears.

### FAIL-26 — block_size not kernel-tile-aligned → internal fragmentation dominates
- Level: SIMPLE
- Pipeline: KV allocation (paging escape-hatch path)
- Axes: leak, jitter
- Scenario: On the paged long-context path, a block_size unaligned to the kernel tile wastes a large fraction of each block on voice's short context (internal frag dominates).
- System must: Choose block_size 8–16 kernel-tile-aligned for the paged path; the per-slot ring stays the default for bounded context (zero reservation waste). (vLLM H-other.)
- If mishandled: The paging path wastes memory and admits fewer long-form streams than it should.

### FAIL-27 — cross-run determinism expected but atomic reductions make it impossible
- Level: SIMPLE
- Pipeline: sampler / reductions
- Axes: numerics, corrupt
- Scenario: A test (or a user) expects bitwise-identical audio across two runs of the same input; non-deterministic atomic reductions on GPU make this impossible and the test flakes / the user files a bug.
- System must: Accept and document per-stream-only determinism (not cross-run bitwise); use float64 Gumbel for the sampler; gate reproducibility tests on per-stream, not cross-run, identity. (vLLM determinism note.)
- If mishandled: Flaky determinism tests and false "non-deterministic output" bug reports.

### FAIL-28 — Gemma-style logits without soft-cap overflow the sampler
- Level: SIMPLE
- Pipeline: AR sampling head (Gemma-class LM)
- Axes: numerics, fail:NaN
- Scenario: A Gemma-class backbone produces logits that, without the tanh soft-cap the model expects, run large enough to destabilize the fp16/fp32 softmax pivot.
- System must: Apply the model's logit soft-cap (tanh) where the architecture declares it; the sampler's fp32 + NaN-safe pivot is the backstop. (vLLM H5.)
- If mishandled: A Gemma-class TTS/STT backbone emits unstable or NaN-tainted tokens.

### FAIL-29 — wall-clock aging absent → a low-priority stream waits forever
- Level: SIMPLE
- Pipeline: admission / fairness
- Axes: hang, fail:hang
- Scenario: Under sustained load a low-priority (or unlucky FCFS) stream is never promoted and waits indefinitely — vLLM has no deadline/aging anywhere (the #41951 omission).
- System must: Promote after `max_wait` (wall-clock aging); FCFS-within-slot-pool + hard per-slot fairness; if priority exists, the comparison key MUST include an age/preemption term. (vLLM H8.)
- If mishandled: Some streams starve forever while others are continuously served.

### FAIL-30 — `enforce_eager` not a first-class knob → no escape when capture fails
- Level: SIMPLE
- Pipeline: kernel selection / OOM ladder
- Axes: CUDA-graph, fail:OOM
- Scenario: CUDA-graph + compile capture costs real memory and can OOM or fail on a given box, but eager isn't a config option, so the only "fix" is a code change.
- System must: Make `enforce_eager` a first-class config knob AND an automatic fallback on capture failure (first rung of the OOM ladder: enforce-eager → cpu/layerwise offload → slicing → reduce shape). (vLLM-Omni OOM ladder, resolves C8.)
- If mishandled: A capture-failing box has no runtime escape and stays down.

---

## INTERMEDIATE — interaction across slots / stages

### FAIL-31 — masked row reads sentinel KV → CUDA illegal-memory kills the whole batch
- Level: INTERMEDIATE
- Pipeline: AR lockstep step
- Axes: KV/state, corrupt, fail:corrupt
- Scenario: An idle/warming slot is left at sentinel `-2` (or stale) input; the dense KV-gather reads an invalid index → CUDA illegal-memory access / NaN that kills all 64 users in the batch, not just the idle one.
- System must: Before the forward, force masked-or-warming rows to the `initial`/BOS token via `where(is_init, initial, gathered)` (`is_init |= ~exec_mask`) so every row has a valid input. Test: a batch with idle slots yields identical output for active slots as if idle slots were absent. (Moshi F1.)
- If mishandled: A single idle slot crashes or poisons every concurrent stream.

### FAIL-32 — ungated per-slot mutation corrupts a stream on idle-then-resume
- Level: INTERMEDIATE
- Pipeline: AR lockstep step
- Axes: KV/state, corrupt, fail:corrupt
- Scenario: One per-stream mutable (RoPE offset, KV scatter, end-offset, conv ring `previous`, sampler RNG offset, partial-word buffer) is updated unconditionally instead of through `where(exec_mask, new, old)`; the corruption only surfaces when a stream idles then resumes (RoPE phase jump / poisoned ring cells) — invisible in single-stream tests.
- System must: Enumerate every mutable per-slot tensor; gate each through exactly ONE masked-select. Test: idle-then-resume a slot, assert byte-identical state + transcript vs a never-idled slot. (Moshi F2.)
- If mishandled: Multi-tenant load silently corrupts streams that pause and resume (e.g. across turns).

### FAIL-33 — ring-KV wraparound mask attends to FUTURE tokens
- Level: INTERMEDIATE
- Pipeline: per-slot ring KV
- Axes: KV/state, corrupt, fail:corrupt
- Scenario: After `offset > context`, physical slot order ≠ time order; a naive causal mask `j ≤ i` now attends to future tokens sitting in recycled physical cells.
- System must: Store a logical position per cell; mask by `pos <= my_pos` (causal) AND window AND never-written⇒-1; bake the Kyutai pre-wrap/exact-fill/post-wrap/mixed-mask test vectors as unit tests. (Moshi F4.)
- If mishandled: Long utterances degrade into incoherent audio once the ring wraps.

### FAIL-34 — slot recycle without reset → cross-user KV contamination
- Level: INTERMEDIATE
- Pipeline: lockstep slot recycling
- Axes: KV/state, contaminate, fail:contaminate
- Scenario: Slot 7's user disconnects, a new user is admitted into slot 7, and without a reset the new user's attention sees the old user's KV + word buffers — cross-user transcript/voice contamination (a privacy breach).
- System must: One transactional `reset_slot(i)` fanning out to KV pointers + conv rings + sampler RNG + word buffers + offset + host state; rely on `positions/indices=0` + mask to make stale bytes unreachable; guard outputs with a monotonic `channel_id` (drop any output whose id ≠ live occupant). Test: a fresh stream in a recycled slot is byte-identical to one in a never-used slot. (Moshi F3.)
- If mishandled: User B hears/sees fragments of User A — a reportable privacy incident.

### FAIL-35 — prefix-cache contamination: wrong voice from a placeholder-ID collision
- Level: INTERMEDIATE
- Pipeline: voice-clone TTS / KV-reuse
- Axes: KV/state, contaminate, fail:contaminate
- Scenario: Voice-clone pastes ref-audio embeds at `-100` placeholder positions; token-ids are identical across requests, so a RadixAttention-style cache concludes prefixes match and reuses one ref-audio's KV for a different ref-audio → silent WRONG-VOICE output, only under concurrency.
- System must: Make the prefix/KV key fingerprint the injected conditioning — `extra_key = blake2b(full N-codebook ref sequence)` over EVERY codebook (cb0-only collides); zero-shot (no ref) ⇒ `extra_key=None` so legit sharing survives. Test: two requests, same text, different ref-audio ⇒ different voices out. (SGLang G1 / literature L1.)
- If mishandled: Cloned-voice users intermittently get someone else's voice.

### FAIL-36 — prefix-cache hash collision → cross-tenant KV leak
- Level: INTERMEDIATE
- Pipeline: hybrid radix/prefix cache (long-form/agent path)
- Axes: KV/state, contaminate, fail:contaminate
- Scenario: The prefix-cache uses a fast non-cryptographic hash (xxhash); two different tenant prefixes collide on the same key → one tenant reads another tenant's cached KV.
- System must: Use sha256 for prefix-cache keys (never xxhash); add a per-tenant `cache_salt` on block-0 to also close the latency side-channel. (vLLM H-other.)
- If mishandled: A hash collision leaks one tenant's context into another's output.

### FAIL-37 — CUDA-graph hang after N requests on sm120
- Level: INTERMEDIATE
- Pipeline: AR lockstep step (graphed) on GB10/sm120
- Axes: CUDA-graph, hang, fail:hang
- Scenario: On sm120 a captured graph hangs after ~6 requests (the documented #40969 sm120 scar) — health still passes, throughput goes to zero.
- System must: Resolve a per-kernel `AttentionCGSupport` MIN across groups and auto-downgrade to eager when a path is unsupported; the progress watchdog (last-audio-emitted) catches the hang and restarts. (vLLM H4 / H9.)
- If mishandled: The server wedges a few requests after every (re)start on the target box.

### FAIL-38 — CUDA-graph capture OOMs AFTER /health passes → crash-loop
- Level: INTERMEDIATE
- Pipeline: graph capture / boot on sm120
- Axes: CUDA-graph, fail:OOM, crash
- Scenario: `/health` passes, then graph capture allocates the pool and OOMs (the #44209 sm120 scar) → the worker crash-loops on the first real request.
- System must: Reserve the CUDA-graph-pool delta BEFORE admitting; run a pre-capture feasibility check at boot (fail at boot, not request-1); fall back to `enforce_eager` automatically on capture failure. (vLLM H3/H4, resolves C8.)
- If mishandled: A box passes health checks then crash-loops the moment traffic arrives.

### FAIL-39 — FULL graph over varlen → silent corruption
- Level: INTERMEDIATE
- Pipeline: graphed encode / EoT / varlen path
- Axes: CUDA-graph, corrupt, fail:corrupt
- Scenario: A FULL CUDA graph is replayed over a variable-length input (the #45425 scar) → silent wrong outputs (no crash).
- System must: Restrict FULL-graph to the fixed/uniform AR lockstep decode (capture exact slot counts 1,2,4…N = zero padding); route audio-encode/varlen/EoT/sampling to eager-or-piecewise; wrap graphed callables with a shape+scalar-identity assert that converts a stale-graph mismatch into a LOUD error. (vLLM H4 / Moshi F7.)
- If mishandled: Variable-length stages emit plausible-but-wrong audio/transcripts with no signal.

### FAIL-40 — `dst.zero_()` skipped → padding writes -1 into a real KV slot
- Level: INTERMEDIATE
- Pipeline: graph padding / slot management
- Axes: KV/state, corrupt, fail:corrupt
- Scenario: Padded slots aren't zeroed before a captured replay; padding writes -1 (or stale) into what is actually a live KV slot (the #43810 scar), corrupting a real stream.
- System must: `dst.zero_()` padded slots before replay; prefer exact-slot-count capture so there is no padding to mismanage; weak-ref graph outputs and freeze GC during capture. (vLLM H4.)
- If mishandled: A padded-slot write silently corrupts a neighboring active stream.

### FAIL-41 — Path-B sidecar SIGKILL → sessions HANG (health still 200)
- Level: INTERMEDIATE
- Pipeline: Path-B torch sidecar
- Axes: crash, hang, fail:hang
- Scenario: The GPU sidecar is SIGKILLed (OOM-killer, operator); the parent still answers `/health` 200 OK while throughput → 0 — built-in health only proves queue-servicing, not GPU progress (#39863).
- System must: 3-tier OS-sentinel detection — (1) `connection.wait([proc.sentinel])` per boundary, (2) an in-band `ENGINE_CORE_DEAD` byte on socket close → client raises EngineDeadError, (3) a 1 s passive sentinel re-poll catches SIGKILL; one `dead` flag drives BOTH admission-reject AND error fan-out into EVERY per-request queue so all sessions fail-fast in ~1 s. (vLLM H6.)
- If mishandled: Every live session hangs indefinitely while the box looks healthy.

### FAIL-42 — orphaned GPU sidecar pins VRAM into the next process
- Level: INTERMEDIATE
- Pipeline: Path-B sidecar lifecycle
- Axes: crash, leak, fail:leak
- Scenario: The parent dies with SIGKILL; a death-pipe Event alone fails because a thread blocked inside a CUDA kernel never polls it → the orphan keeps running and pins VRAM, so the next worker can't allocate (#34643).
- System must: `prctl(PR_SET_PDEATHSIG, SIGTERM)` at sidecar entry (kernel-guaranteed even under parent SIGKILL); teardown order = abort-collectives BEFORE destroy, SIGTERM→5s→SIGTERM→4s→SIGKILL. (vLLM H7.)
- If mishandled: VRAM leaks across restarts until the GPU is unusable without a reboot.

### FAIL-43 — alive-but-zero-forward-progress passes every health check
- Level: INTERMEDIATE
- Pipeline: any stage / progress
- Axes: hang, fail:hang
- Scenario: A loop is alive and servicing its queue but emits no audio (stuck inner solve, deadlocked marker heap) — the #39863 blind spot that no liveness/queue health check catches.
- System must: Maintain a monotonic "last-audio-emitted-at T" per session, checked by an independent watchdog thread; no audio for > N×frame-interval on an ACTIVE session ⇒ kill/restart the stage. (vLLM H9, resolves C5-adjacent.)
- If mishandled: Sessions silently emit nothing while every dashboard is green.

### FAIL-44 — out-of-order: vocoder gets a stream-chunk before its own payload
- Level: INTERMEDIATE
- Pipeline: stage-DAG (AR → vocoder, parallel paths)
- Axes: streaming, corrupt
- Scenario: On parallel paths with no cross-path ordering, a vocoder receives an AR stream-chunk before its own request payload arrives → it has no codec contract yet → silent corruption if it proceeds.
- System must: Make pre-payload stream acceptance EXPLICIT opt-in (`can_accept_stream_before_payload`), else hard-fail; latch the codec contract from whichever of {payload | chunk-meta} arrives first; monotone `chunk_id` per (req,target). (SGLang out-of-order rule.)
- If mishandled: The vocoder decodes against an undefined contract and emits garbage.

### FAIL-45 — cancelled stream looks identical to a completed one
- Level: INTERMEDIATE
- Pipeline: barge-in / cancellation
- Axes: streaming, corrupt
- Scenario: Barge-in cancels a TTS stream, but the terminal frame is the same as a normal completion; downstream (and the client) can't distinguish an interrupted utterance from a finished one.
- System must: Emit a DISTINGUISHABLE terminal frame for cancel vs complete (cancelled ≠ completed); barge-in is a control message that jumps every stage's queue and frees the slot/KV/window within ≤1 tick. Test: cancelled and completed streams produce different terminal frames. (SGLang G2 / §6 barge-in.)
- If mishandled: A barged-in turn is treated as completed (or replayed), confusing the dialog state.

### FAIL-46 — fire-and-forget abort DROPS the cancel for a late stage
- Level: INTERMEDIATE
- Pipeline: cancellation fan-out across stages
- Axes: streaming, hang
- Scenario: Abort is a PUB/SUB broadcast; a stage that connects late never receives an abort published before it joined (ZMQ PUB drops to not-yet-connected SUBs); the `sleep(0.1)` "give subscribers time" is a band-aid → the barge-in is lost for that stage.
- System must: Use a reliable abort channel with per-stage ACK (not best-effort PUB/SUB); one terminal failure/cancel aborts the request across ALL stages (fail-fast). (SGLang G9.)
- If mishandled: Barge-in is silently ignored by one stage, which keeps generating stale audio.

### FAIL-47 — relay credit deadlock: wait-then-notify on a pull transport
- Level: INTERMEDIATE
- Pipeline: sidecar↔stage relay (NIXL/RDMA/shm)
- Axes: leak, hang, fail:hang
- Scenario: On a receiver-initiates-read transport, the producer awaits transfer completion BEFORE sending the data-ready control message → the receiver never learns there's data to pull → deadlock.
- System must: Notify-before-wait — send the data-ready CONTROL message BEFORE awaiting completion; make this a tested per-transport property; treat a double credit-release as a HARD error, not a swallowed log. (SGLang G4.)
- If mishandled: The pipeline deadlocks under load with no error, just silence.

### FAIL-48 — unbounded per-request bookkeeping leak on a long-lived server
- Level: INTERMEDIATE
- Pipeline: scheduler bookkeeping
- Axes: leak, fail:leak
- Scenario: Sets/maps like `_closed`, `_aborted`, `_stream_chunk_counters` grow per request and are never trimmed; on a 24/7 voice server they leak until OOM.
- System must: Cap every per-request bookkeeping set/map (e.g. 10000 → trim-5000); purge per-slot maps on slot-free; keep the ordered egress queue unbounded only with sender-side credit backpressure. (SGLang G6.)
- If mishandled: Slow memory growth → an OOM days into an uptime with no single culprit.

### FAIL-49 — shm orphan leak when the receiver crashes
- Level: INTERMEDIATE
- Pipeline: shared-memory relay
- Axes: leak, crash, fail:leak
- Scenario: shm is receiver-owns-unlink; the receiver crashes before unlinking → the shm segment leaks and accumulates across crashes.
- System must: Run an orphan-reaper for shm segments (and a credit-pool reset) so a crashed receiver's segments are reclaimed. (SGLang G4.)
- If mishandled: `/dev/shm` fills up over time, eventually failing all allocations.

### FAIL-50 — fan-out by reference aliases mutable state across stages
- Level: INTERMEDIATE
- Pipeline: in-process stage fan-out (1 result → N stages)
- Axes: KV/state, contaminate, fail:contaminate
- Scenario: A same-process fan-out shares one payload by reference (or `Arc<Mutex<Payload>>`); a mutation in one downstream stage corrupts the others.
- System must: Move ownership across in-process channels (`Box<Payload>`); clone-on-fan-out the owned container, share `Arc` only for immutable tensor leaves; serialize only cross-process. (SGLang G5.)
- If mishandled: One stage's in-place edit silently corrupts a sibling stage's input.

### FAIL-51 — codec stage inherits AR batch size → audio gaps under concurrency
- Level: INTERMEDIATE
- Pipeline: stage-DAG (AR → codec)
- Axes: streaming, jitter, fail:corrupt
- Scenario: A uniform `max_num_seqs` default sets the codec stage to the AR stage's batch size; the codec window round-robins across requests and produces audible gaps under concurrency (RFC #2568).
- System must: Pin per-stage batch defaults — AR `≥4` to pipeline, codec `=1`; the codec micro-batch stage never inherits the AR batch size. (vLLM-Omni codec rule, resolves C6.)
- If mishandled: Concurrent TTS streams develop periodic dropouts only under load.

### FAIL-52 — Path-B sidecar holds Python codec state un-keyed → crosstalk under load
- Level: INTERMEDIATE
- Pipeline: Path-B sidecar (codec/sliding-window/generator state)
- Axes: KV/state, contaminate, fail:contaminate
- Scenario: The torch sidecar caches streaming-generator / codec / sliding-window-pad state on the model object, shared across `forward()` calls without keying by slot — concurrent requests cross-talk (crosstalk/truncation only under load).
- System must: Key sidecar state by slot-id (`self._state: dict[slot_id, State]`) and free on slot-reset; add a concurrent-crosstalk test gate. (vLLM-Omni I5, resolves C3.)
- If mishandled: Concurrent sidecar sessions bleed audio into each other.

### FAIL-53 — fan-in deadlock when a conditional branch won't fire
- Level: INTERMEDIATE
- Pipeline: stage-DAG fan-in (STT→translate→TTS / thinker→talker→vocoder)
- Axes: hang, fail:hang
- Scenario: A fixed `wait_for=[a,b,c]` deadlocks when a request's branch never fires (text-only request → no audio-encoder output ever arrives).
- System must: Compute the expected source set per request via `wait_for_fn(req,…) → expected_sources`; constrain `route_fn` to the statically-declared `next` (ValueError otherwise) and forbid empty routes; support multi-terminal merge for text+audio S2S. (SGLang G11.)
- If mishandled: Any request that skips a branch hangs the merge stage forever.

### FAIL-54 — prefill spike breaks frame cadence (TBT 28×, 17–22 dropped frames)
- Level: INTERMEDIATE
- Pipeline: prefill firewall / admission
- Axes: jitter, fail:hang
- Scenario: A new stream's prefill is naively co-batched with running decodes; per-token TBT inflates up to 28.3× (P99 TBT 1.76 s ≈ 17–22 dropped frames at an 80 ms budget) → total dropout for everyone.
- System must: Admit ≤1 new stream's prefill per K frames; chunk any prefill exceeding one frame-budget's tokens (token budget keyed on the audio frame deadline); keep chunk token counts power-of-two (257 is ~32% slower than 256 — tile quantization). (§4.5 prefill firewall.)
- If mishandled: One new connection causes a synchronized dropout across all active streams.

### FAIL-55 — power-of-two-plus-one tile-quantization cliff
- Level: INTERMEDIATE
- Pipeline: graph capture / batched-token sizing
- Axes: jitter, corrupt
- Scenario: A captured/batched shape lands at 257 (or 272/512+1) tokens and runs ~32% slower than 256 due to wave/tile quantization, silently eating the frame margin.
- System must: Capture exact slot counts (1,2,4…N) for zero padding and keep chunk token counts power-of-two; never let a 257-shaped batch sneak past the budget check. (vLLM H4 / §4.5.)
- If mishandled: An off-by-one shape pushes a previously-safe batch over budget.

### FAIL-56 — one stage dies, no isolation → whole process group exits
- Level: INTERMEDIATE
- Pipeline: multi-stage process group
- Axes: crash, fail:crash
- Scenario: Stages are colocated in one process group with no per-stage failure isolation; a flaky encoder raises and takes down the hot AR stage and every co-resident session.
- System must: Run the hot AR stage and the flaky encoder as SEPARATE processes; exclusivity invariants are AssertionErrors, not comments; the 3-layer crash detection (scheduler-thread handler, background-task done-callbacks, 5 s process-liveness) fails in-flight requests and pushes a failure CompleteMessage into every stream-queue. (SGLang G7.)
- If mishandled: An encoder hiccup becomes a full-server outage.

### FAIL-57 — silent background-task death wedges a stage
- Level: INTERMEDIATE
- Pipeline: stage background task
- Axes: hang, crash, fail:hang
- Scenario: A stage's background task dies silently (exception swallowed by the executor); the stage stops making progress but nothing surfaces it.
- System must: Attach done-callbacks to every background task (silent task death wedges the stage otherwise) and surface it via the crash-detection fan-out. (SGLang G7 layer-2.)
- If mishandled: A stage quietly stops working with no crash and no log.

### FAIL-58 — async/overlap fast-path enabled at bs=1 (net-negative + double-free)
- Level: INTERMEDIATE
- Pipeline: AR decode pipelining
- Axes: corrupt, leak
- Scenario: One-step-lookahead async decode is on at bs=1 (the voice-heavy case); its fixed event/pingpong/bookkeeping overhead costs more than it saves, and a stale-batch overrun re-runs a finished request → double-frees its KV.
- System must: Gate pipelining/double-buffering on `batch_size ≥ 2` (fall back to the synchronous path at bs=1, matching CUDA-graph-hurts-@high-batch); any in-flight-step optimization must handle abort/finish-during-overrun (filter_batch guard). (SGLang G8.)
- If mishandled: Single-call latency regresses and a finished request's KV is double-freed (use-after-free).

### FAIL-59 — startup error reported as "didn't become ready" (no traceback)
- Level: INTERMEDIATE
- Pipeline: sidecar/stage startup
- Axes: crash, hang
- Scenario: A child stage fails during init; the parent only sees "didn't become ready" with no child traceback → undiagnosable boot failures.
- System must: Carry the startup error back over a dedicated channel (child traceback), not a generic readiness timeout. (SGLang G7.)
- If mishandled: Boot failures are opaque and slow to triage.

### FAIL-60 — masked-idle slots silently pace all streams to the slowest
- Level: INTERMEDIATE
- Pipeline: lockstep batch under heterogeneous residency
- Axes: jitter, leak
- Scenario: With barge-in/EOS/VAD creating variable residency, fixed-slot exec-mask institutionalizes slowest-stream-paces-all; idle lanes still consume bandwidth/energy (padding 13%@BS1 → 40%@BS32; idle-lane ~48% of serving energy).
- System must: Compact/repack active slots when residency is heterogeneous, OR explicitly budget the masked-slot bandwidth/energy cost in admission (masked slots are NOT free under variable residency). (literature L8.)
- If mishandled: A few stalled slots silently throttle throughput and waste GPU on dead lanes.

### FAIL-61 — long-form context overruns the ring → silent forgetting
- Level: INTERMEDIATE
- Pipeline: per-slot ring KV (long-form TTS / long-audio STT / many-turn)
- Axes: KV/state, corrupt
- Scenario: A 10-min stream pushes context past the ring size; the sliding window forgets older tokens and wraps unstably (StreamingLLM forgetting without a pinned attention-sink), degrading coherence.
- System must: Pin attention-sink tokens; provide a paged/full-context escape hatch for long-form streams (the ring is the fast path for bounded context, not a silent truncator). (literature L12.)
- If mishandled: Long sessions drift into incoherence with no signal that context was lost.

### FAIL-62 — variable-stride / variable-inner-NFE streams can't share a lockstep tick
- Level: INTERMEDIATE
- Pipeline: AR-outer + generative-inner head (DiTAR/FlashTTS/CALM class)
- Axes: corrupt, jitter
- Scenario: Streams run patch-AR (advance by a patch, not a frame) or a per-stream runtime NFE dial (10 vs 2); a fixed single-stride/single-NFE lockstep tick can't co-batch them and either stalls or mis-aligns.
- System must: Generalize lockstep to "advance a model-dependent VARIABLE STRIDE"; compose the inner variable-NFE diffusion/flow micro-batch INSIDE one AR step (the nested batcher composes two batchers per step, not picks one). (literature L5.)
- If mishandled: Modern AR+inner-head models either can't batch or emit misaligned audio.

### FAIL-63 — dynamic frame-rate codec breaks fixed-rate cohorting
- Level: INTERMEDIATE
- Pipeline: cohort batching (FlexiCodec-class, 3–12.5 Hz data-dependent)
- Axes: corrupt, jitter
- Scenario: A codec's frame-rate is data-dependent per-utterance and per-frame (not known a-priori); the `(model, frame_rate)` cohort key is undefined and lockstep mixes incompatible clocks.
- System must: Make the cohort key tolerate unknown-a-priori rates and have lockstep advance a variable stride; never lockstep-mix two different realtime clocks within a fused step. (literature L6.)
- If mishandled: Variable-rate codecs corrupt the batch or force every stream to a single process.

### FAIL-64 — relay fan-out by reference between micro-batch and streaming batches mixed
- Level: INTERMEDIATE
- Pipeline: micro-batch collector (vocoder/CFM)
- Axes: corrupt, jitter
- Scenario: The micro-batch collector mixes streaming and non-streaming requests in one batch, so a streaming request inherits the wrong egress semantics (or vice-versa).
- System must: Never mix streaming and non-streaming in a batch; default vocoder micro-batch `max_batch_size=4, max_batch_wait_ms=2`; bucket by (model, latent-shape, step-schedule). (SGLang out-of-order rule / §4.2.)
- If mishandled: A streamed request gets batched as one-shot and the listener hears a single late blob.

### FAIL-65 — ECC/Xid GPU fault mid-stream
- Level: INTERMEDIATE
- Pipeline: any GPU stage
- Axes: GPU-fault, crash, fail:crash
- Scenario: An uncorrectable ECC error / Xid fault hits the GPU mid-utterance; subsequent kernels may return garbage or the context becomes invalid.
- System must: Detect the fault (driver error code / failed launch), fail all in-flight sessions on that device fast via the dead-flag fan-out, drain the device, restart the stage on a healthy context; never emit post-fault garbage frames. (vLLM H6 + GPU-fault recovery.)
- If mishandled: The engine streams corrupted audio after an unrecoverable GPU fault.

### FAIL-66 — multinomial-in-CUDA-graph determinism drift across CFG-parallel
- Level: INTERMEDIATE
- Pipeline: step-bucket batcher (CFG-folded)
- Axes: numerics, corrupt
- Scenario: The CFM/diffusion step is run CFG-parallel without a seeded generator; cond and uncond diverge from the sequential reference → non-deterministic, occasionally-wrong audio.
- System must: Pass a seeded `generator` to the scheduler step; fold CFG into the batch dim (×2) deterministically; accept per-stream-only determinism (bitwise cross-run is impossible due to atomic reductions) with float64 Gumbel. (vLLM-Omni CFG rule / vLLM determinism note.)
- If mishandled: CFG-parallel output drifts from the reference and is irreproducible.

### FAIL-67 — acoustic-delay off-by-one collides write and read in the codebook ring
- Level: INTERMEDIATE
- Pipeline: multistream delay engine / RQ-Transformer
- Axes: KV/state, corrupt
- Scenario: The per-codebook acoustic-delay ring is sized `max_delay` (not `max_delay+2`); the max-delay write and the oldest read collide → a codebook reads a half-written cell.
- System must: Size the cache depth `max_delay+2` (the +2 off-by-one guard); write `(offset+delays[k])%CT`, read `(offset-max_delay+gen_delays[k])%CT`; teacher-force codebooks≥1 to PAD before `step < acoustic_delay`. Test the delay write/read alignment. (Moshi F8.)
- If mishandled: Multi-codebook TTS emits glitches at the start of every utterance.

### FAIL-68 — exec-mask init-token trap: warming slot embedded before its first real token
- Level: INTERMEDIATE
- Pipeline: AR lockstep step (slot admission)
- Axes: KV/state, corrupt, fail:corrupt
- Scenario: A freshly-admitted slot is still in its prefill/warm-up window but the loop treats it as active and embeds a not-yet-valid gathered token, polluting its first KV cells.
- System must: Fold warming rows into the same `is_init |= ~exec_mask` substitution (BOS/initial token) used for masked rows; a slot contributes real tokens only after its warm-up completes. (Moshi F1.)
- If mishandled: Every new stream starts from a corrupted first frame.

### FAIL-69 — semantic-VAD / end-of-turn head misfires → premature slot free or runaway turn
- Level: INTERMEDIATE
- Pipeline: extra heads (semantic-VAD / EoT) on the AR spine
- Axes: streaming, corrupt
- Scenario: The per-step EoT/VAD linear head fires early (cuts the user off mid-word) or never (the turn runs away and the slot is held); both break duplex turn-taking.
- System must: Treat the EoT/VAD head as a generic per-step head with hysteresis; free the slot only after offset ≥ real end (NEVER on a single EoT tick or on disconnect alone, tail may still drain); model PAD/EPAD/SILENCE state tokens for robust turn boundaries. (Moshi F5/F7 + literature L7.)
- If mishandled: The bot interrupts the user or won't stop talking.

### FAIL-70 — async turn-taking / barge-in creates variable residency the fixed batch mishandles
- Level: INTERMEDIATE
- Pipeline: full-duplex lockstep under barge-in
- Axes: KV/state, jitter
- Scenario: Barge-in, EOS, and VAD silence make stream lifetimes highly variable; a "no length variance in voice" assumption mis-sizes slots and lets a long-residency stream pace short ones.
- System must: Handle heterogeneous residency first-class — model the user stream every frame (barge-in is always-modeled), use SILENCE/PAD state tokens, and compact/budget idle lanes (FAIL-60) rather than assume fixed residency. (literature L7 + L8.)
- If mishandled: Jittery turn-taking and idle-lane waste under real conversational load.

### FAIL-71 — detokenizer/text-decode stall blocks the audio stream
- Level: INTERMEDIATE
- Pipeline: STT/S2S text egress + audio egress
- Axes: streaming, jitter, fail:hang
- Scenario: A shared/blocking detokenizer (text post-processing) stalls and back-pressures the audio path, so a text hiccup degrades audio cadence.
- System must: Decouple text detokenization from the audio egress path (separate stage/queue); per-request detokenizer-cache batching keeps it off the audio hot loop (VoxServe). (literature L3 / VoxServe.)
- If mishandled: A slow text decode audibly stutters the audio stream.

### FAIL-72 — wave-quantization: fused batch width misaligned to GB10 SM tiles
- Level: INTERMEDIATE
- Pipeline: prefill firewall / fused batch sizing on GB10
- Axes: jitter, corrupt
- Scenario: The fused batch (chunk-prefill + piggybacked decodes) isn't aligned to the GB10 SM/tile geometry, leaving SMs idle in the last wave (~19.4% SM-idle wave-quantization) and silently eating the frame margin.
- System must: Align the fused batch width to GB10 tiles; use a KV-length-aware PREDICTED-LATENCY budget (not raw token-count) for the prefill firewall so attention cost is accounted. (literature L10 + Bullet.)
- If mishandled: Wave-quantization wastes ~1/5 of the GPU and pushes safe batches over budget.

### FAIL-73 — step-bucket assumes fixed-N/CFG-folded but the model has per-request variable NFE
- Level: INTERMEDIATE
- Pipeline: step-bucket batcher (flow/diffusion heads)
- Axes: corrupt, jitter
- Scenario: A bucket keyed on fixed-N + CFG-folded can't batch IntMeanFlow (NFE=1 feedforward), LLaDA-TTS (cost=T independent of length), or CFG-free flow — these don't fit the bucket and either fall back to bs=1 or mis-batch.
- System must: Make the bucket key accept per-request variable N (including N=1), length-decoupled step counts, and mixed trajectories; CFG-folding is not universal — gate it on the model. (literature L15.)
- If mishandled: Newer flow/consistency TTS heads can't batch and run at bs=1 latency.

### FAIL-74 — MTP/Depformer heads emit but the rectangular lockstep isn't preserved
- Level: INTERMEDIATE
- Pipeline: acoustic AR path with MTP (VocalNet/Qwen3-TTS/FlashTTS class)
- Axes: corrupt, jitter
- Scenario: Multi-token-prediction (2–3 tokens/step) is bolted on in a way that varies per-stream token counts, breaking the rectangular batch — or someone reaches for EAGLE/Medusa draft-spec-decode, which destroys lockstep and is a 0.98× slowdown on acoustic tokens.
- System must: Treat the Depformer/code-predictor AS the MTP mechanism (direct-emit, fixed heads) which PRESERVES rectangular lockstep; explicitly do NOT add EAGLE/Medusa on the acoustic path. (literature L14 + L13.)
- If mishandled: MTP destroys batchability, or draft-spec-decode slows the TTS path.

### FAIL-75 — Batch priority not piggybacked into leftover budget → wasted frame slack
- Level: INTERMEDIATE
- Pipeline: mixed Realtime+Batch admission
- Axes: jitter, leak
- Scenario: Realtime streams leave per-frame compute slack but Batch work isn't piggybacked into it, so offline/batch jobs starve while the GPU idles within the frame budget.
- System must: Priority `Realtime > Batch` per stage with Sarathi-style piggyback of Batch into leftover budget; Batch never delays a Realtime frame but fills the gaps. (§6 priority.)
- If mishandled: Batch throughput collapses even though Realtime leaves the GPU partly idle.

---

## COMPOUND — two faults compose

### FAIL-76 — slot recycle + ungated mutation → privacy leak that survives the reset
- Level: COMPOUND
- Pipeline: lockstep slot recycle + per-slot mutation
- Axes: KV/state, contaminate, corrupt, fail:contaminate
- Scenario: Slot 7 recycles to User B (FAIL-34) AND one per-slot mutable (sampler RNG offset or word buffer) is ungated (FAIL-32); the transactional reset misses the ungated field, so a fragment of User A's state survives into User B's stream.
- System must: The single `reset_slot()` must fan out to EVERY enumerated per-slot mutable (the same enumeration FAIL-32 requires); a reset-completeness test asserts the recycled slot is byte-identical to a never-used slot across ALL fields, not just KV. (Moshi F2 + F3.)
- If mishandled: The "reset" looks complete but leaks the one field nobody gated.

### FAIL-77 — NaN frame during a CUDA-graph replay
- Level: COMPOUND
- Pipeline: graphed AR step
- Axes: numerics, CUDA-graph, fail:NaN, fail:corrupt
- Scenario: A logit goes NaN (FAIL-1) on a step that is currently a captured graph replay; the NaN guard's reject path may itself involve control flow the graph can't express, so the reject is skipped and the garbage token is emitted anyway.
- System must: Keep the NaN check + reject-frame OUTSIDE the captured region (post-graph, like the sampler in FAIL-5); the graph emits raw logits, the eager wrapper inspects + rejects. (vLLM H1 + Moshi F7 / vLLM-Omni I-CUDA-graph.)
- If mishandled: Graph mode silently bypasses the NaN guard and pops.

### FAIL-78 — prefill spike + thermal throttle simultaneously blow the budget
- Level: COMPOUND
- Pipeline: admission + AR stage
- Axes: jitter, fail:hang
- Scenario: A new-stream prefill (FAIL-54) lands in the same window the GPU starts thermal-throttling (FAIL-23); each alone was within margin, together they overrun and drop frames for all streams.
- System must: Admission tests LIVE measured step time (not the calibration constant) AND the per-K-frame prefill quota together; if the combined duty exceeds S, REJECT the new stream rather than admit-and-glitch. (§4.5 + §6 + drift response.)
- If mishandled: A new connection during a warm spell synchronizes a fleet-wide dropout.

### FAIL-79 — sidecar OOM during graph capture (don't-kill-the-world)
- Level: COMPOUND
- Pipeline: Path-B sidecar + graph capture
- Axes: fail:OOM, CUDA-graph, crash
- Scenario: The sidecar OOMs while capturing the graph pool (FAIL-38) AND the host OOM-killer targets a neighboring co-resident model, so a transient capture spike kills an unrelated healthy model.
- System must: Reserve the graph-pool delta before admitting; on OOM, fall back to `enforce_eager` for the capturing model ONLY (don't kill the world); a per-model memory budget prevents one capture from starving co-residents. (vLLM H3/H4 + OOM ladder.)
- If mishandled: One model's capture spike takes down an unrelated tenant's model.

### FAIL-80 — dead sidecar + unbounded egress queue → memory spike on top of the crash
- Level: COMPOUND
- Pipeline: sidecar crash + egress
- Axes: crash, leak, fail:OOM
- Scenario: The sidecar dies (FAIL-41) but, before the dead-flag fan-out fires, producers keep enqueueing into unbounded egress queues (FAIL-11/41) → a memory spike piled on top of the crash.
- System must: The `dead` flag must be consulted by BOTH admission AND every live WS send/enqueue (single source of truth); bounded drop-oldest queues cap the spike; fail-fast in ~1 s before the queues grow. (vLLM H2 + H6.)
- If mishandled: A crash turns into a crash + OOM that takes longer to recover.

### FAIL-81 — barge-in cancel races slot-recycle → output lands in the wrong session
- Level: COMPOUND
- Pipeline: cancellation + slot recycle
- Axes: streaming, contaminate, fail:contaminate
- Scenario: Barge-in cancels User A and frees slot 7 (FAIL-45); User B is admitted into slot 7 in the same tick; a late in-flight frame from A's pipeline is delivered to B's session.
- System must: Guard every output with a monotonic `channel_id`; drop any output/marker whose id ≠ the live occupant (FAIL-34's channel-id guard); cancel frees the slot only after the channel-id is bumped. (Moshi F3 + SGLang G2/G9.)
- If mishandled: A leftover frame from the cancelled user is injected into the next user's audio.

### FAIL-82 — fan-in deadlock + missed abort → stage hangs and can't be cancelled
- Level: COMPOUND
- Pipeline: stage-DAG fan-in + cancellation
- Axes: hang, fail:hang
- Scenario: A conditional branch never fires so the merge stage waits forever (FAIL-53) AND the barge-in abort is dropped because the stage connected late (FAIL-46) → the session is wedged with no way out.
- System must: Dynamic `wait_for_fn` prevents the deadlock AND the reliable ACK'd abort channel guarantees the cancel reaches the stage; the progress watchdog is the final backstop that kills the wedged stage. (SGLang G11 + G9 + vLLM H9.)
- If mishandled: A text-only request that gets barged-in hangs permanently.

### FAIL-83 — quant divergence only on the long-context paging path + spec-decode
- Level: COMPOUND
- Pipeline: token-AR STT paging + sparse-KV spec-decode
- Axes: quant, corrupt, fail:corrupt
- Scenario: Long-audio STT uses the paged KV escape hatch with sparse-KV spec-decode (allowed only there); a quant variant that passed short fixtures diverges on the long context where the sparse-KV draft mispredicts.
- System must: The accuracy gate must include a LONG-context + streaming fixture for the paging path; scope spec-decode strictly to the long-context token-AR-STT path (never on the acoustic AR-TTS path, where it's a 0.98× slowdown). (literature L13 + I4.)
- If mishandled: Long-form transcripts degrade where the short-fixture gate never looked.

### FAIL-84 — ring wraparound + slot recycle in the same long session
- Level: COMPOUND
- Pipeline: ring KV + slot recycle (many-turn agent)
- Axes: KV/state, corrupt, contaminate, fail:corrupt
- Scenario: A long many-turn session wraps the ring (FAIL-33) AND a co-tenant churns the adjacent slot (FAIL-34); a wraparound mask bug + an incomplete reset compound into cross-cell, cross-user reads.
- System must: Logical-position-per-cell masking AND transactional reset must both hold; the Kyutai wraparound test vectors run alongside the reset-completeness test so the compound case is covered, not just each alone. (Moshi F3 + F4.)
- If mishandled: Long sessions next to churning slots intermittently read the wrong user's wrapped KV.

### FAIL-85 — prefix-cache contamination + hash collision under multi-tenant clone
- Level: COMPOUND
- Pipeline: hybrid radix/prefix cache (multi-tenant voice-clone)
- Axes: KV/state, contaminate, fail:contaminate
- Scenario: Two tenants clone different voices; a placeholder-ID collision (FAIL-35) AND a weak-hash collision (FAIL-36) both point at the same cache entry → a near-certain wrong-voice cross-tenant leak.
- System must: The prefix key is sha256 over the full N-codebook ref sequence PLUS a per-tenant `cache_salt` — both the content fingerprint and the tenant salt must differ for a hit. Test: same text, same nominal ref, different tenant ⇒ no shared entry. (SGLang G1 + vLLM H-other.)
- If mishandled: A multi-tenant clone service leaks voices across tenants.

### FAIL-86 — slot leak on disconnect + bookkeeping leak compound to faster exhaustion
- Level: COMPOUND
- Pipeline: slot lifecycle + bookkeeping
- Axes: leak, fail:leak
- Scenario: Missed disconnect callbacks leak slots (FAIL-12) AND per-request maps aren't trimmed (FAIL-48); together the server exhausts slots and memory in tandem under churn.
- System must: Multi-trigger slot-free AND capped/purged bookkeeping (purge keyed on slot-free) so a freed slot also purges its maps in one path. (Moshi F9 + SGLang G6.)
- If mishandled: Churn-heavy traffic exhausts both slots and memory together.

### FAIL-87 — NCCL-destroy hang during teardown after one stage already died
- Level: COMPOUND
- Pipeline: multi-GPU/multi-process teardown
- Axes: crash, hang, fail:hang
- Scenario: One stage died (FAIL-56); during teardown the collectives are destroyed before being aborted → NCCL-destroy hangs (#43413) → the whole shutdown wedges and the orphan keeps VRAM.
- System must: Abort-collectives BEFORE destroy; cleanup-budget > kill-floor; SIGTERM→5s→SIGTERM→4s→SIGKILL; bounce the signal handler to a thread to avoid handler-mutex deadlock. (vLLM H7.)
- If mishandled: A single stage death wedges shutdown and leaks VRAM into the next process.

### FAIL-88 — admission accepts on AR-fit but the bottleneck codec can't sustain
- Level: COMPOUND
- Pipeline: admission + bottleneck stage
- Axes: jitter, fail:hang
- Scenario: Admission checks only the AR stage's free slot (FAIL-51's cousin) AND the codec/CFM bottleneck stage's duty is already at S; the stream is admitted, the bottleneck overruns, and frames drop for everyone on that stage.
- System must: Admission tests the BOTTLENECK stage's duty (per-substrate duty ledger), not the AR stage; reject if admitting breaks ANY stage's frame budget. (§6 bottleneck admission.)
- If mishandled: The codec/vocoder stage silently overloads while AR looks fine.

### FAIL-89 — zero-copy placement + shared-bandwidth oversubscription on GB10
- Level: COMPOUND
- Pipeline: heterogeneous placement on unified memory
- Axes: jitter, fail:hang
- Scenario: The placer moves the codec to the NPU to free GPU bandwidth (zero-copy, FAIL-free in transfer) BUT both the AR (GPU) and codec (NPU) now saturate the single ~273 GB/s LPDDR ceiling they share → both stages miss deadlines.
- System must: Budget aggregate memory bandwidth as a schedulable resource (the shared-bandwidth ledger); admit only if `Σ bandwidth_duty ≤ S·ceiling`; prefer to overlap a memory-bound stage with a compute-bound one, co-locate + time-share when both saturate bandwidth. (§3.4 contention guard + §6.)
- If mishandled: "Freeing the GPU" oversubscribes the shared bus and degrades both stages.

### FAIL-90 — transport reconnect + missing-FINAL → duplicate or truncated audio
- Level: COMPOUND
- Pipeline: transport reconnect + streaming contract
- Axes: transport, streaming, corrupt
- Scenario: A WebSocket drops and reconnects mid-stream; without a clear delta cursor + FINAL contract, the client either replays already-played audio or loses the tail.
- System must: Delta-only egress with a resumable chunk cursor + explicit FINAL (FAIL-7 + FAIL-22); on reconnect, resume from the acknowledged delta offset, never re-send cumulative; a reconnect that never sees FINAL is a failure. (vLLM-Omni I1 + SGLang G2.)
- If mishandled: Reconnects cause audible replays or dropped endings.

### FAIL-91 — head-of-line block: long audio-prompt encode stalls all streams
- Level: COMPOUND
- Pipeline: encoder stage + admission
- Axes: jitter, transport, fail:hang
- Scenario: A long audio-prompt encode (e.g. a 60 s reference clip) runs unchunked on the shared encoder and head-of-line-blocks every other stream's encode (#37308: 147× TTFT HoL).
- System must: Force-chunk long audio-prompt encode (`long_prefill_token_threshold`); run the hot AR stage and the encoder as separate processes so a slow encode can't pin the AR loop. (vLLM H-other + SGLang G3.)
- If mishandled: One long reference clip stalls every concurrent session's first audio.

### FAIL-92 — jitter buffer underrun + step overrun on the telephony egress
- Level: COMPOUND
- Pipeline: telephony egress (8 kHz, 20 ms RTP) + AR stage
- Axes: jitter, transport, fail:hang
- Scenario: A step overrun (FAIL-20) coincides with a thin jitter buffer on the 20 ms RTP repacketizer → the buffer underruns and the PSTN side hears a gap.
- System must: Size the playback/jitter buffer to absorb transient step overruns; repacketize to fixed 20 ms RTP via the jitter buffer; sustained overrun trips drift response before the buffer can't cover it. (§5.1 + §6.)
- If mishandled: Telephony callers hear gaps whenever a step blips.

### FAIL-93 — calibration stamp mismatch after a driver upgrade → silent over-admission
- Level: COMPOUND
- Pipeline: calibration lifecycle + admission
- Axes: jitter, corrupt
- Scenario: A driver/warm-set change invalidates the persisted calibration (`sha256 × device × driver × warm-set`), but admission keeps using the stale duty numbers → it over-admits and overruns.
- System must: Key the calibration stamp on device+driver+warm-set; on mismatch, re-run calibration (gated behind `/readyz`) before admitting; never admit against a stale stamp. (§6 calibration + C7.)
- If mishandled: A driver bump silently makes admission over-optimistic and starts dropping frames.

### FAIL-94 — MIG repartition mid-flight invalidates capacity assumptions
- Level: COMPOUND
- Pipeline: GPU partitioning + admission
- Axes: GPU-fault, jitter
- Scenario: An operator repartitions MIG on the device (or a fractional GPU is resized) while streams are live; the slot/KV/bandwidth budgets computed for the old partition are now wrong.
- System must: Re-probe device capability + re-calibrate on a partition change (treat it like a driver change, FAIL-93); drain to the new capacity rather than over-admit against stale budgets. (§2 backend probe + §6.)
- If mishandled: Post-repartition admission over-commits the smaller partition and glitches.

### FAIL-95 — poison-pill flood + bookkeeping leak amplify each other
- Level: COMPOUND
- Pipeline: ingress validation + bookkeeping
- Axes: crash, leak, fail:leak
- Scenario: A tenant floods malformed requests (FAIL-17); each rejected request still allocates a bookkeeping entry that isn't trimmed (FAIL-48) → the flood leaks memory even though every request is rejected.
- System must: Reject poison-pills at ingress BEFORE allocating per-request bookkeeping; cap + trim any bookkeeping that is allocated; rate-limit a flooding tenant. (vLLM-Omni I2 + SGLang G6.)
- If mishandled: A reject-everything flood still OOMs the server via bookkeeping growth.

### FAIL-96 — warm-start race: two replicas capture graphs and OOM together
- Level: COMPOUND
- Pipeline: fleet rollout + graph capture
- Axes: CUDA-graph, fail:OOM, crash
- Scenario: A rollout brings two replicas up on the same box simultaneously; both capture graph pools at once and jointly OOM (neither alone would).
- System must: Serialize/stagger graph capture across co-resident replicas; pre-capture feasibility check accounts for co-resident pools; warm over-provisioning, never scale-to-zero/cold-start. (vLLM H3/H4 + literature L9.)
- If mishandled: A rollout double-captures and OOMs both new replicas.

### FAIL-97 — overrun on the bottleneck during a KV-migration spill
- Level: COMPOUND
- Pipeline: DC spill/rebalance + bottleneck stage
- Axes: jitter, fail:hang
- Scenario: A Llumnix-style KV migration moves a stream between replicas, but one decode-step > one frame, so the migration drops ≥1 frame UNLESS playback-buffer-masked — and it lands during a bottleneck-stage overrun.
- System must: Mask migration drops with the client playback buffer (sub-ms–5 ms KV transfer for voice ctx); only migrate when the target replica's bottleneck duty has headroom; never migrate into an already-saturated bottleneck. (literature L16 + §6.)
- If mishandled: Spill/rebalance audibly glitches the migrated stream.

### FAIL-98 — barge-in cancels the LLM but a stale fast-tier token still fires
- Level: COMPOUND
- Pipeline: realtime-reasoning two-tier (fast + reasoning LLM) + TTS
- Axes: streaming, corrupt
- Scenario: Barge-in cancels the in-flight reasoning LLM, but a parallel-fired fast-tier token (or a buffered TTS frame) is already in the codec pipeline and gets spoken after the user interrupted.
- System must: Barge-in is one control message that cancels the LLM AND flushes the TTS pipeline AND bumps the channel-id within ≤1 tick; the fast tier's committed audio is dropped, not spoken. (§6 barge-in + REALTIME_REASONING invariants.)
- If mishandled: The bot keeps talking over a user who already interrupted.

### FAIL-99 — empty-tensor KV dtype mismatch on the q4f16 CUDA path
- Level: COMPOUND
- Pipeline: quantized (q4f16) AR on CUDA + KV-cache init
- Axes: quant, corrupt, crash
- Scenario: A q4f16-on-CUDA variant needs the empty KV-cache tensors (enc pkv, past_padding_cache, dec zero_past) in f16, but they're hardcoded f32; the dtype mismatch crashes or silently mis-types the graph (the documented voxtral q4f16 seam).
- System must: Make empty-tensor / KV-cache dtype graph-driven via `StaticGraph::input_types()` (input_features/inputs_embeds/audio_embeds stay f32; KV-init follows the weight precision); argmax-last already handles f16 logits. (voxtral q4f16 finding, generalizable.)
- If mishandled: The "zero-code weight swap" to q4f16 crashes or emits wrong audio on CUDA.

### FAIL-100 — reconnect storm overwhelms admission after a transient network blip
- Level: COMPOUND
- Pipeline: transport reconnect + admission
- Axes: transport, jitter, fail:hang
- Scenario: A network blip drops 64 streams at once; all 64 reconnect within a second (a thundering herd) and each re-prefills, breaching the per-K-frame prefill quota en masse.
- System must: Reject-with-Retry-After under the prefill quota (reconnect storms are absorbed by admission, not the AR loop); jittered client backoff; warm capacity holds the herd rather than cold-starting. (§4.5 + §6 + literature L9.)
- If mishandled: A blip becomes a self-inflicted prefill stampede and a fleet-wide dropout.

### FAIL-101 — graph-disabled-by-corruption + bs=1 sync stall = double latency hit on edge
- Level: COMPOUND
- Pipeline: edge single-stream (graphed AR + sidecar)
- Axes: CUDA-graph, jitter, fail:hang
- Scenario: A FULL-over-varlen corruption (FAIL-39) forces a graph downgrade to eager on the edge box, AND the sidecar still runs a per-step D2H sync (FAIL-8); the edge stream loses both the 1.21× graph win and pays the sync stall — now sub-realtime at bs=1.
- System must: The eager downgrade must keep the sync-free per-step loop (the two are independent); zero-D2H-sync holds in eager too, so losing the graph costs only ~1.21×, not the budget. (vLLM H4 + vLLM-Omni I3.)
- If mishandled: A graph downgrade compounds with a sync stall and pushes the edge box sub-realtime.

### FAIL-102 — long-form ring overrun + KV-quant eviction = compounding forgetting in a multi-turn agent
- Level: COMPOUND
- Pipeline: long many-turn S2S (ring KV + KV-quant)
- Axes: KV/state, quant, corrupt
- Scenario: A multi-turn agent wraps the ring (FAIL-61) AND uses permanent KV-quant eviction (RocketKV-style) to fit streams; the two compound so earlier-turn context is doubly lost and the agent forgets across turns.
- System must: Pin attention-sink tokens AND prefer head-split KV-quant (DuoAttention) over permanent eviction for multi-turn; provide the paged full-context escape hatch for genuinely long sessions. (literature L12 + L16.)
- If mishandled: A multi-turn agent silently forgets earlier turns under memory pressure.

### FAIL-103 — intra-node spatial P/D contention: prefill pool steals SMs from the decode pool
- Level: COMPOUND
- Pipeline: intra-node spatial prefill/decode partition (GB10)
- Axes: jitter, fail:hang
- Scenario: An intra-node spatial P/D split (the un-evaluated competitor to the chunked-prefill firewall) is mis-partitioned so the prefill pool steals SM share from the decode pool during a prefill burst → decode (the frame clock) misses deadlines — the very TBT spike spatial P/D was meant to avoid.
- System must: A/B the intra-node SM-partition against the chunked-prefill firewall on GB10; size the decode partition to protect the frame clock first; chunked-prefill remains the default until spatial P/D is measured to win the frame-deadline metric. (literature L4 + §4.5.)
- If mishandled: A "P/D optimization" reintroduces the >8× TBT decode spike it claimed to fix.

### FAIL-104 — calibration under profiler + first-request lazy-init = wrong admission ceiling
- Level: COMPOUND
- Pipeline: calibration harness + admission
- Axes: jitter, corrupt
- Scenario: Calibration is run with the torch profiler attached (distorts latency — "Command Buffer Full" is profiler overhead) AND includes the first-request lazy-init step; both inflate the measured `T_step`, so admission computes a too-low ceiling and under-utilizes the box (or, if measured low elsewhere, over-admits).
- System must: Measure `T_step` WITHOUT a profiler and EXCLUDE the first (warm-up) request; calibration runs under synthetic co-load only after warmup gates readiness. (vLLM-Omni diffusion-perf P0 + §6.)
- If mishandled: The admission ceiling is wrong from boot — either wasted capacity or chronic overruns.

### FAIL-105 — vendored upstream scheduler touched internals → de-facto fork breaks on update
- Level: COMPOUND
- Pipeline: vendored components (moshi-core / candle / parakeet-rs / upstream scheduler)
- Axes: crash, corrupt
- Scenario: A vendored upstream scheduler/component is integrated by touching internals (subclassing, reaching past public methods); a later upstream update changes those internals → silent breakage or a crash that's a de-facto fork to maintain.
- System must: Integrate by COMPOSITION + no-op stubs + PIN the version (delegate to public methods only; stub off detokenizer/grammar/spec-decode/LoRA for the voice path); a content-sniffer that walks the object graph carries a `seen` set (cycle-safe). (SGLang G10.)
- If mishandled: Every upstream bump risks a hard-to-trace break in vendored code.

### FAIL-106 — out-of-order pre-payload stream + micro-batch streaming/non-streaming mix
- Level: COMPOUND
- Pipeline: vocoder micro-batch + parallel-path ingest
- Axes: streaming, corrupt
- Scenario: A vocoder receives AR stream-chunks before its payload (FAIL-44) AND the micro-batch collector mixes that streaming request with a non-streaming one (FAIL-64); the streaming request inherits one-shot egress and arrives as a late blob with the wrong codec contract.
- System must: Pre-payload acceptance is explicit opt-in with the codec contract latched from the first arrival; the micro-batch NEVER mixes streaming and non-streaming; monotone `chunk_id` per (req,target) orders the chunks. (SGLang out-of-order rule + §4.2.)
- If mishandled: A streamed clone arrives as a single late blob, voiced from the wrong contract.

---

## EXTREME — cascading multi-subsystem failure

### FAIL-107 — NaN frame during a graph replay on sm120 while the sidecar OOMs under a poison-pill clone flood in a flash crowd
- Level: EXTREME
- Pipeline: full stack (AR graphed + sidecar + admission + cache + transport)
- Axes: numerics, CUDA-graph, fail:OOM, crash, contaminate, fail:NaN
- Scenario: The headline cascade — a tenant floods poison-pill voice-clone requests (FAIL-17/28) during a flash crowd (FAIL-100); the resulting load triggers a sidecar graph-capture OOM on sm120 (FAIL-38/64); mid-replay a logit goes NaN (FAIL-77); the OOM-killer eyes a co-resident model; and a placeholder-ID collision threatens a wrong-voice leak (FAIL-35).
- System must: Each defense holds INDEPENDENTLY and composes — ingress rejects poison-pills before allocation; admission reject-with-Retry-After absorbs the flash crowd; the graph-pool reservation + pre-capture check + auto-eager fallback prevents the capture OOM from spreading (don't-kill-the-world); the post-graph NaN guard rejects the frame; the sha256+salt+full-codebook prefix key blocks the wrong-voice leak; the dead-flag fan-out + progress watchdog bound any residual hang to ~1 s. The cascade degrades to "reject + eager + reject-frame," never to corruption or a hang. (vLLM H1/H3/H4/H6 + Moshi F3/F7 + SGLang G1 + §6.)
- If mishandled: Simultaneous pop + wrong-voice leak + crash-loop + fleet-wide hang — the worst-case incident.

### FAIL-108 — one stage dies → NCCL teardown hangs → orphan pins VRAM → restart OOMs → crash-loop
- Level: EXTREME
- Pipeline: multi-process stage group lifecycle
- Axes: crash, hang, leak, fail:OOM
- Scenario: A flaky encoder raises (FAIL-56); teardown destroys collectives before aborting → NCCL-destroy hangs (FAIL-87); the orphaned GPU sidecar keeps VRAM (FAIL-42); the supervisor restarts but the new process OOMs on the pinned VRAM → crash-loop.
- System must: 3-layer crash detection fails in-flight requests immediately; abort-collectives-before-destroy + `PR_SET_PDEATHSIG` guarantee the orphan dies and releases VRAM; the restart waits on confirmed VRAM reclamation (sentinel) before re-capturing; bounded restart backoff prevents a tight crash-loop. (SGLang G7 + vLLM H6/H7.)
- If mishandled: One encoder bug cascades into a permanent VRAM-leaking crash-loop needing a reboot.

### FAIL-109 — slot churn + ring wrap + incomplete reset + co-tenant flood = cross-user privacy breach under load
- Level: EXTREME
- Pipeline: lockstep slots + ring KV + multi-tenant admission
- Axes: KV/state, contaminate, corrupt, leak, fail:contaminate
- Scenario: A multi-tenant flash crowd churns slots rapidly (FAIL-86); long sessions wrap the ring (FAIL-33); one per-slot field is ungated (FAIL-32) so the reset is incomplete (FAIL-76); the channel-id guard is the last line — and a late frame races a recycle (FAIL-81).
- System must: Defense-in-depth holds — full per-slot mutable enumeration + ONE transactional reset (byte-identical to never-used) + logical-position wraparound masking + the monotonic channel-id output guard (drop any frame whose id ≠ live occupant) together guarantee no cross-user leak even under churn; the reset-completeness + wraparound test vectors + recycle-race test cover the compound. (Moshi F2/F3/F4 + SGLang G2.)
- If mishandled: A flash crowd produces a multi-user privacy breach (User B hears User A) — the highest-severity outcome.

### FAIL-110 — thermal throttle + prefill storm + bottleneck saturation + bandwidth oversubscription on GB10
- Level: EXTREME
- Pipeline: full GB10 heterogeneous DAG under load
- Axes: jitter, fail:hang
- Scenario: A sustained flash crowd heats the GB10 (FAIL-23); reconnect/prefill storms hit the quota (FAIL-54/85); the codec bottleneck saturates (FAIL-88); and the codec-on-NPU placement oversubscribes the shared ~273 GB/s bus (FAIL-89) — four budget pressures at once on one shared ceiling.
- System must: ONE coherent admission decision integrates all four — live measured step time (thermal), per-K-frame prefill quota (storm), bottleneck-stage duty (codec), and the shared-bandwidth ledger (NPU+GPU) — and REJECTS rather than admits when any budget is breached; drift response sheds Batch then newest Realtime ≤1/tick with hysteresis; warm capacity absorbs the crowd. No frames are dropped for already-admitted streams. (§4.5 + §6 + §3.4.)
- If mishandled: Four small overshoots compound into a fleet-wide, self-amplifying dropout.

### FAIL-111 — sidecar dies mid-utterance → dead-flag race → unbounded queue spike → OOM-killer hits a co-tenant → cascading model loss
- Level: EXTREME
- Pipeline: sidecar crash + egress + co-resident models
- Axes: crash, leak, fail:OOM, hang
- Scenario: The sidecar SIGKILLs mid-utterance (FAIL-41); before the dead-flag propagates, producers spike unbounded queues (FAIL-80); the host OOM-killer, seeing the spike, kills a co-resident healthy model (FAIL-79) → a second outage cascades from the first.
- System must: Single dead-flag consulted by admission AND every enqueue stops new work in ~1 s; bounded drop-oldest queues cap the spike so the OOM-killer is never provoked; per-model memory budgets isolate co-residents (one model's failure can't starve another); 3-tier sentinel + progress watchdog bound the blast radius. (vLLM H2/H6 + OOM ladder.)
- If mishandled: One sidecar death cascades into an unrelated tenant's model being killed.

### FAIL-112 — variable-stride model + dynamic frame-rate codec + CFG non-determinism + long-context wrap, all in one S2S session
- Level: EXTREME
- Pipeline: AR-outer/generative-inner + dynamic codec + long-form S2S
- Axes: numerics, KV/state, corrupt, jitter, fail:corrupt
- Scenario: A many-turn S2S session uses a FlashTTS-class model (variable stride + variable inner-NFE, FAIL-62) with a FlexiCodec dynamic frame-rate (FAIL-63), CFG run parallel without a seeded generator (FAIL-66), and a context long enough to wrap the ring (FAIL-61) — four lockstep/correctness assumptions broken at once.
- System must: The generalized loop holds — variable-stride advance + composed inner variable-NFE micro-batch + cohort key tolerant of unknown-a-priori rates + seeded deterministic CFG + pinned attention-sink with a paged escape hatch — so the session stays coherent and reproducible across turns. (literature L5/L6/L12 + vLLM-Omni CFG rule.)
- If mishandled: Modern S2S models drift, mis-batch, or become irreproducible in long sessions — the engine can't run SOTA voice.

### FAIL-113 — calibration-stamp-stale after driver upgrade → over-admission → thermal throttle → bottleneck overrun → silent fleet degradation
- Level: EXTREME
- Pipeline: rollout/calibration + admission + thermal + bottleneck
- Axes: jitter, corrupt, fail:hang
- Scenario: A driver upgrade invalidates calibration (FAIL-93) but admission keeps the stale optimistic duty; it over-admits; the over-admission heats the GPU (FAIL-23); the bottleneck codec overruns (FAIL-88) — a slow, silent, fleet-wide degradation with no single crash to point at.
- System must: Calibration stamp keyed on device+driver+warm-set FORCES re-calibration behind `/readyz` after the upgrade (the rollout can't serve on a stale stamp); thereafter live-measured-step-time admission + bottleneck duty + drift response self-correct. The first line of defense is refusing to admit against an invalid stamp. (§6 calibration + drift response + C7.)
- If mishandled: A routine driver bump silently degrades an entire fleet with no obvious cause.

### FAIL-114 — observability blind spot: every dashboard green while audio is silently corrupt
- Level: EXTREME
- Pipeline: full stack observability
- Axes: corrupt, hang, fail:corrupt
- Scenario: The compound nightmare for operators — a NaN-reject path is firing (FAIL-1), a quant variant is MOS-degraded (FAIL-18), and a stage makes zero forward progress (FAIL-43), yet liveness/queue/health are all 200 and CPU/GPU look busy.
- System must: First-class SEMANTIC metrics make the invisible visible — `last-audio-emitted-at` per session (progress), `waav_quant_gate_failed` + MOS-on-fixtures (quality), NaN-reject-frame counter (numerics), per-step-wall-time + per-stream-buffer-depth (cadence), used/total_slots (capacity) — and the progress watchdog + accuracy gate ACT on them, not just chart them. (vLLM H9 + vLLM-Omni I4 + Moshi F9/F10.)
- If mishandled: Production ships silently-bad audio for hours because nothing health-checks the actual output.

### FAIL-115 — admit-and-degrade at 50% overload (the policy that loses 80% of SLOs)
- Level: EXTREME
- Pipeline: overload / admission policy
- Axes: jitter, fail:hang
- Scenario: Under a sustained 50% overload, a naive engine admits everything and degrades all streams (vLLM admit-then-preempt) → 80% SLO violations and synchronized dropouts, versus deadline-aware relegation's 8.6%.
- System must: Deadline-aware admission as the PRIMARY mechanism — reject/relegate to a degraded queue by risk-of-violation (Niyama/VoxServe binary-viability), protect cadence with the client playback buffer, hard-reject only at true saturation; shed is the backstop, admission is the mechanism. (§6 graceful overload + literature L9.)
- If mishandled: An overload event degrades everyone instead of cleanly serving the streams that fit.

### FAIL-116 — rolling deploy mid-utterance: drain vs hard-cut + warmup cliff on the new replica
- Level: EXTREME
- Pipeline: lifecycle / rollout
- Axes: crash, hang, jitter
- Scenario: A rolling deploy drains the old replica while utterances are in flight AND routes new traffic to a not-yet-warm new replica (FAIL-15); a hard-cut truncates live audio and the warmup cliff stalls the first new-replica streams.
- System must: Short-drain-then-abort on the old replica (never hard-cut mid-utterance, never unbounded drain); the new replica's `/readyz` gates on warmup+calibration so traffic only arrives warm; barge-in/cancel frees slots cleanly during drain. (vLLM H7 + Moshi F6 + C7.)
- If mishandled: Every deploy truncates in-flight calls and stalls the first users on the new version.

### FAIL-117 — colocation starvation cascade: AR loop pins the core, encoder + codec both miss deadlines, admission still says yes
- Level: EXTREME
- Pipeline: colocated multi-stage single process on GB10
- Axes: jitter, hang, fail:hang
- Scenario: A cost-driven colocation puts the hot AR loop, an encoder, and a codec in one process; a busy AR loop starves the core (FAIL-13), the encoder slows ~600×, a long audio-prompt encode head-of-line-blocks (FAIL-91), the codec window round-robins into gaps (FAIL-51), and admission — checking only the AR stage — keeps admitting (FAIL-88).
- System must: Compose the defenses — block-on-idle (never busy-spin), force-chunk long encodes, hot AR vs encoder as SEPARATE processes (colocation is an optimization that MUST be starvation-load-tested), per-stage codec batch=1, and BOTTLENECK-stage admission via the per-substrate duty ledger; the safe default is stage=process for the hot AR vs encoder. (SGLang G3/G7 + vLLM-Omni codec rule + §6.)
- If mishandled: One colocation decision cascades into encoder starvation, HoL blocking, codec gaps, and runaway admission — a self-inflicted brownout.

### FAIL-118 — full-duplex S2S meltdown: barge-in storm + EoT misfires + delay-ring off-by-one + variable residency under a flash crowd
- Level: EXTREME
- Pipeline: full-duplex Moshi-class S2S at scale
- Axes: KV/state, streaming, corrupt, jitter, fail:contaminate
- Scenario: A flash crowd of duplex calls produces a barge-in storm; EoT heads misfire (FAIL-69), the per-codebook delay ring has an off-by-one (FAIL-67), variable residency paces the batch (FAIL-70), a barge-in races a slot recycle (FAIL-81), and a dropped abort (FAIL-46) leaves a stage talking over an interrupted user.
- System must: The full-duplex contract holds end-to-end — user stream always-modeled per frame, EoT with hysteresis + free-only-after-tail-drains, delay ring sized max_delay+2, idle-lane budgeting, monotonic channel-id output guard on recycle, and a reliable ACK'd abort that cancels every stage within ≤1 tick; the progress watchdog backstops any residual hang. Degrades to clean turn-taking + rejects, never to crosstalk or a bot talking over the user. (Moshi F2/F3/F5/F8 + SGLang G2/G9 + §6 barge-in + literature L7.)
- If mishandled: A duplex flash crowd produces crosstalk, talked-over users, and start-of-utterance glitches simultaneously — the duplex worst case.

---

## Coverage

This catalog enumerates **118 distinct failure/recovery scenarios** for WaaV Infer, graded SIMPLE (30) → INTERMEDIATE (45) → COMPOUND (31) → EXTREME (12), each a concrete production break with its optimal KISS recovery and the cost of mishandling. Numbering is contiguous (FAIL-1…FAIL-118) in escalating order; compound/extreme scenarios cite the simpler scenarios they compose. Coverage spans every axis the brief required:

- **Numerics** (`fail:NaN`, `corrupt`): NaN/Inf-logit → reject-frame (the top inversion, H1), fp16 softmax overflow → prefer bf16, tiny-temp exp blow-up + `_MAX_TEMP` clamp, all-masked logits → ≥1-survivor guarantee, multinomial-in-CUDA-graph → sample-outside-or-gumbel-argmax, fp32 sampler/CFM guards, Gemma logit soft-cap, CFG-parallel determinism + per-stream-only reproducibility.
- **KV/state** (`contaminate`, `corrupt`): MASKED≠ABSENT input-substitution (F1), exec-mask init-token trap, every-per-slot-mutation-gated on idle-then-resume (F2), ring-wraparound logical-position mask (F4), transactional slot-recycle vs stale-KV cross-user contamination (F3), prefix-cache wrong-voice/ref-audio contamination (G1/L1), sha256+salt cross-tenant-collision, padding-into-real-slot, fan-out aliasing, sidecar Python-state crosstalk (I5), long-form ring overrun + attention-sink, acoustic-delay off-by-one (F8), q4f16 empty-KV dtype.
- **CUDA-graph (GB10/sm120)** (`CUDA-graph`): capture-OOM-after-health-pass (#44209), hang-after-N-requests (#40969), FULL-over-varlen silent corruption (#45425), tile/wave-quantization (257≠256) cliffs, `dst.zero_()` padded-slot, capability-min eager fallback + first-class `enforce_eager` (H4/C8), exact-slot capture, sampler-outside-graph.
- **Crash / recovery** (`crash`, `hang`, `leak`): sidecar SIGKILL → 3-tier OS-sentinel + ENGINE_CORE_DEAD byte + dead-flag fan-out (H6), orphan → PR_SET_PDEATHSIG + abort-collectives-before-destroy (H7), one-stage-dies isolation + 3-layer detection (G7), NCCL-destroy hang, OOM-don't-kill-the-world per-model budget, GPU ECC/Xid, MIG repartition, startup-traceback channel.
- **Progress** (`hang`): alive-but-zero-forward-progress watchdog keyed on last-audio-emitted (H9), device+model-aware 1–5 s deadline (not flat 300 s), silent background-task death callbacks.
- **Streaming** (`streaming`, `corrupt`): delta-not-cumulative invariant (I1/C1), marker-drop truncation (F5), explicit FINAL / done≠stalled (G2), cancelled≠completed, out-of-order pre-payload opt-in, fire-and-forget-abort drop → ACK'd channel (G9), barge-in-cancels-LLM-and-flushes-TTS, detokenizer-stall decoupling.
- **Resource / leak** (`leak`, `fail:OOM`): slot-leak multi-trigger (F9), unbounded bookkeeping caps (G6), HWM=0 → bounded drop-oldest (H2), relay credit deadlock notify-before-wait (G4), shm orphan-reaper, masked-idle-slot energy/bandwidth budget (L8), `empty_cache` hot-path stall.
- **Transport** (`transport`, `jitter`): WS write-coalescing flush-per-frame (F10), reconnect resume-from-delta-offset, head-of-line long-encode chunking (#37308), jitter-buffer underrun on 20 ms RTP, reconnect storm → Retry-After.
- **Resource / admission** (`jitter`): KV-length-aware prefill firewall + per-K-frame quota (§4.5/L10), bottleneck-stage duty-ledger admission (§6), shared-bandwidth oversubscription guard on GB10 (§3.4), calibration-stamp staleness (driver/MIG), KV-migration spill playback-masked (L16), graceful-overload-relegate-not-reject (L9), Realtime>Batch piggyback, control/data-plane separation, wall-clock aging (H8).
- **Quant accuracy** (`quant`, `corrupt`): MOS-crash/WER-flat perceptual gate (I4/C4), int8-on-ORT-CUDA 20× latency loss → per-EP precision routing (§5.2), long-context paging divergence, q4f16 graph-driven dtype, autocast-corrupts-codec fp32 rule.
- **GPU faults** (`GPU-fault`): thermal throttle → live-measured-step admission, ECC/Xid fast-fail, MIG repartition re-probe.
- **Modern-arch breakage** (literature L5/L6/L14/L15): variable-stride + variable-inner-NFE lockstep, dynamic frame-rate cohorting, MTP-via-Depformer (no EAGLE/Medusa on acoustic), variable-N step-bucket, heterogeneous-residency duplex (L7).
- **Poison-pill** ingress validation and **clock-overrun** explicit policy are first-class throughout.

The EXTREME tier delivers the requested cascading failures, headlined by **FAIL-107** (a NaN frame during a sm120 CUDA-graph replay while a sidecar OOMs under a poison-pill voice-clone flood in a flash crowd) and the cross-user-privacy compound **FAIL-109**, with **FAIL-118** as the full-duplex-S2S meltdown — each showing the independent defenses composing so the system degrades to "reject + eager + reject-frame," never to corruption or a hang. Every scenario maps to a cited scar (vLLM-core H1–H9, Moshi F1–F10, SGLang-Omni G1–G11, vLLM-Omni I1–I5, literature L1–L16) and the §2–§6 architecture, so this family doubles as the production-hardening test backlog.
