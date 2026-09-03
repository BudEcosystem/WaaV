# 06 — Batching / Scheduling / Worker / Stage-DAG Scenarios

> Family scope: lockstep fixed-slot AR batching, step-bucket / micro-batch / nested batchers, cohort-by-(model,frame-rate), the heterogeneous stage-DAG with pipeline overlap + back-pressure, heterogeneous placement + shared-bandwidth duty, multi-worker fan-out, CUDA-graph capture per slot-count cohort, the bs=1 fast-path, masked-idle-slot economics, the prefill firewall, and non-preemptible whole-stream admission.
> Grounding: `INFER_ENGINE.md` §3 (stage-DAG), §4 (batching), §6 (scheduler); failure catalog Moshi F1–F10, SGLang G1–G11, vLLM-core H1–H9, literature L1–L16.
> Axis legend — `batch:lockstep|step-bucket|micro|cohort|nested`, `worker:multi`, plus situational tags (`mask`, `slot-recycle`, `back-pressure`, `placement`, `zero-copy`, `cuda-graph`, `prefill`, `admission`, `duty-ledger`, `bs1`, `variable-stride`, `variable-nfe`, `compaction`, `saturation`).

---

## SIMPLE — single-mechanism, steady-state

### BAT-1 — Admit one stream into a free lockstep slot
- Level: Simple
- Pipeline: AR codec-LM TTS (lockstep stage)
- Axes: batch:lockstep, admission
- Scenario: A single TTS request arrives at an idle worker; the lockstep batcher has B_max free slots and must seat the new stream.
- System must: Pick the lowest free slot index, run `prefill(slot, conditioning)`, set the slot's exec-mask bit true, and begin ticking it on the next frame boundary — no queue, no ledger entry beyond the slot reservation.
- If mishandled: The stream either never starts (no slot assigned) or starts mid-tick and skips its first frame.

### BAT-2 — All slots idle: short-sleep instead of busy-spin
- Level: Simple
- Pipeline: any lockstep stage
- Axes: batch:lockstep, mask
- Scenario: No streams are active; the exec-mask is all-false for several consecutive ticks.
- System must: Apply admissions/control-plane first, compute exec-mask, and if `!exec_mask.any()` short-sleep 1–2 ms (block on a Notify/recv_timeout) — never launch the kernel on an all-false batch, never busy-spin the core (Moshi F6, SGLang G3).
- If mishandled: A co-located encoder/CFM stage is starved of CPU/launch budget (~600× slowdown observed in SGLang), or the GPU runs a useless all-masked forward every tick.

### BAT-3 — Steady 4-stream lockstep tick
- Level: Simple
- Pipeline: AR codec-LM TTS
- Axes: batch:lockstep
- Scenario: Four same-model streams run concurrently at 12.5 Hz; every tick all four emit one frame.
- System must: Gather the 4 active rows into the rectangular `[B_max,…]` batch, run one masked step, scatter per-slot frames to per-slot output queues, advance each slot's offset — exploiting the flat batch-1→64 decode cost (~free batching).
- If mishandled: Per-stream sequential decode wastes the GPU (4× the necessary wall time) and overruns the frame budget under load.

### BAT-4 — bs=1 fast-path: skip the overlap machinery
- Level: Simple
- Pipeline: AR codec-LM (single live call)
- Axes: batch:lockstep, bs1
- Scenario: Exactly one stream is active on the worker (the common single-call case).
- System must: Take the synchronous bs=1 path — no async one-step-lookahead, no double-buffer pingpong (net-negative at bs=1, SGLang G8; `async_decode_min_batch_size=2`); and prefer the CUDA-graphed step (1.21× at batch-1).
- If mishandled: Fixed event/bookkeeping overhead of the overlap path makes the single most common workload slower, not faster.

### BAT-5 — Cohort key separates two frame-rates
- Level: Simple
- Pipeline: two TTS models (12.5 Hz Mimi-class + 25 Hz)
- Axes: batch:cohort
- Scenario: Two streams want service: one on a 12.5 Hz model, one on a 25 Hz model.
- System must: Place them in distinct `(model, frame_rate)` cohorts and tick each on its own clock — never fuse them into one lockstep step (a 12.5 Hz and 25 Hz stream share no common realtime tick, §4.2).
- If mishandled: A fused step paces both at the slower clock (the faster stream underruns) or aliases the two phase clocks into audible jitter.

### BAT-6 — Micro-batch the codec/vocoder stage at 2 ms deadline
- Level: Simple
- Pipeline: codec_stream / vocoder
- Axes: batch:micro
- Scenario: Several AR streams emit token-frames into the shared codec stage within the same window.
- System must: Drain the codec inbox up to `max_batch_size` within a ~2 ms deadline, run one length-bucketed graphed decode, and stream delta samples — the codec stage owns its own scheduler, decoupled from the AR clock (§3.2).
- If mishandled: Waiting for a full batch adds latency; running each frame singly wastes the (already efficient) vocoder; mixing it onto the AR tick couples the two clocks.

### BAT-7 — Step-bucket a CFM denoiser by length bucket
- Level: Simple
- Pipeline: flow/CFM TTS head
- Axes: batch:step-bucket
- Scenario: Three chunk-CFM requests arrive with the same NFE schedule but different latent lengths.
- System must: Bucket by `(model, latent-shape, step-schedule, CFG)`, fold CFG cond+uncond into the batch dim (×2), precompute the timestep schedule/masks once, and run the fixed N forwards with a static graph per step.
- If mishandled: Re-deriving the schedule per request, or putting different-length latents in one rectangular batch, wastes compute or crashes on shape mismatch.

### BAT-8 — Codec stage uses batch=1, AR uses ≥4 (decoupled sizes)
- Level: Simple
- Pipeline: 2-node AR→codec DAG
- Axes: batch:lockstep, batch:micro
- Scenario: One uniform `max_num_seqs` default is tempting for both stages.
- System must: Pin per-stage batch independently — AR `≥4` to pipeline, codec `=1` (RFC #2568 / catalog C6); the codec micro-batch stage must NOT inherit the AR batch size.
- If mishandled: A uniform default of 1 everywhere causes audio gaps under concurrency because the codec window round-robins across requests.

### BAT-9 — Bounded inter-stage queue parks the upstream, never drops
- Level: Simple
- Pipeline: AR→codec DAG
- Axes: back-pressure
- Scenario: The codec stage falls momentarily behind; its bounded inbox fills.
- System must: Park (back-pressure) the upstream AR stage on the full downstream queue — never drop frames, never reorder; admission already tested the bottleneck stage so this is transient (§3.2).
- If mishandled: Dropping a frame = audible glitch; an unbounded queue silently accumulates GBs of stale audio (vLLM H2 HWM=0 footgun).

### BAT-10 — Slot recycling resets all per-slot state transactionally
- Level: Simple
- Pipeline: lockstep AR stage
- Axes: batch:lockstep, slot-recycle
- Scenario: A user disconnects from slot 7; a new user is admitted into slot 7.
- System must: Run ONE transactional `reset_slot(7)` fanning out to KV pointers, conv rings, sampler RNG, partial-word buffers, offset, and host item-state before the new user's first step (Moshi F3); does not need to wipe KV bytes if positions/indices=0 make stale bytes unreachable.
- If mishandled: The new user's attention sees the old user's KV/word buffer = cross-user transcript contamination (privacy disaster).

### BAT-11 — Stale-output guard via monotonic channel-id
- Level: Simple
- Pipeline: batched ASR / AR stage
- Axes: batch:lockstep, slot-recycle
- Scenario: A delayed frame/marker for the previous occupant of a slot arrives after the slot was recycled.
- System must: Stamp every output and marker with the slot's monotonic `channel_id` and drop any whose id ≠ the live occupant's (Moshi F3, `batched_asr.rs:92-100`).
- If mishandled: A late frame from the prior stream is delivered to the new user (crosstalk / wrong transcript fragment).

### BAT-12 — One CUDA graph lasts the worker's lifetime
- Level: Simple
- Pipeline: lockstep AR stage
- Axes: cuda-graph, batch:lockstep
- Scenario: Streams come and go but the active slot count stays at the captured cohort size because idle slots are masked, not removed.
- System must: Keep B, T=1, and cache shapes constant by masking-not-dropping idle slots so a single captured graph replays forever (Moshi F7) — capture exact slot counts (1,2,4,…N) with zero padding (vLLM H4).
- If mishandled: Dropping idle slots changes B every frame → re-capture every tick = catastrophic; or padding to 257/512 hits the power-of-two tile cliff.

### BAT-13 — Warm-up the graph off the hot path, gate readiness on it
- Level: Simple
- Pipeline: any lockstep / batched stage
- Axes: cuda-graph, admission
- Scenario: A worker boots and must capture its CUDA graph before serving.
- System must: Run 2–3 warm-up steps with a full mask + `synchronize()` at startup to fill conv/KV boundary state and force capture OFF the hot path, and gate `/readyz` on warm-up+calibration complete — not process-up (Moshi F6, catalog C7).
- If mishandled: The first real request pays seconds of capture latency (the first-request cliff), or `/readyz` returns 200 before warm-up and admits a request into an uncaptured engine.

### BAT-14 — Masked rows get a substituted valid input token
- Level: Simple
- Pipeline: lockstep AR stage
- Axes: batch:lockstep, mask
- Scenario: Some rows of the rectangular batch are idle/warming this tick.
- System must: Force masked-or-warming rows to the `initial`/BOS token via `where(is_init, initial, gathered)` BEFORE the embedding/KV-gather (Moshi F1) — masked ≠ absent; the dense kernel reads every row.
- If mishandled: The KV-gather reads sentinel −2 / stale memory → CUDA illegal-memory or NaN that kills the whole batch (all 64 users).

### BAT-15 — Gate every per-stream mutation through the exec-mask
- Level: Simple
- Pipeline: lockstep AR stage
- Axes: batch:lockstep, mask
- Scenario: Each tick mutates offset, KV scatter, conv ring `previous`, the `first`-frame flag, and the sampler RNG offset per slot.
- System must: Update every mutable per-slot tensor through exactly one `where(exec_mask, new, old)` masked-select (Moshi F2) — enumerate them and gate each.
- If mishandled: A single ungated mutation corrupts a slot that idles-then-resumes (RoPE phase jump / poisoned ring cells) — invisible in single-stream tests, breaks under multi-tenant load.

### BAT-16 — Idle-then-resume a slot byte-identically
- Level: Simple
- Pipeline: lockstep AR stage
- Axes: batch:lockstep, mask
- Scenario: A stream goes silent (VAD gap) for several ticks while masked, then resumes.
- System must: Keep its KV/conv/offset frozen (all mutations masked off) so on resume its state is byte-identical to a never-idled stream and its RoPE phase continues correctly.
- If mishandled: The resumed stream's audio is corrupted by an offset/phase drift that only manifests after an idle gap.

### BAT-17 — Free a slot only after the tail drains, not on disconnect
- Level: Simple
- Pipeline: batched ASR (delay pipeline)
- Axes: batch:lockstep, slot-recycle
- Scenario: A client disconnects but the model still owes the last few delayed frames (asr_delay + buffered_frames).
- System must: Model the ACTIVE→MARKER_RECEIVED→IS_EOS lifecycle and free the slot only after offset ≥ real_end, never on disconnect alone (Moshi F5).
- If mishandled: The slot is freed mid-drain → the last words are truncated, or freed-then-reused while the tail is still emitting.

### BAT-18 — Disable WS write-coalescing on streaming routes
- Level: Simple
- Pipeline: streaming egress
- Axes: back-pressure
- Scenario: The default socket coalesces small frames to batch writes.
- System must: Set `write_buffer_size(0)` (flush per frame) on every streaming route so the 80 ms frame budget isn't eroded by tens of ms of coalescing jitter (Moshi F10).
- If mishandled: Frames arrive bunched → playback jitter / underrun even though the engine met its compute budget.

### BAT-19 — Inline mode: no scheduler, no ledger, no tick loop
- Level: Simple
- Pipeline: single-stream edge DAG
- Axes: admission
- Scenario: `mode=edge` or a single stream with no co-tenant.
- System must: Run all stages inline on the calling thread in DAG order at B=1, with nested loops still in-forward, and spin up NO queues/tick-loop/admission/ledger — the edge never pays for DC machinery (§8 Inline mode).
- If mishandled: Standing up the full duty ledger + tick loop for one stream adds pure overhead and latency on the edge.

### BAT-20 — `used/total_slots` gauge as the autoscale signal
- Level: Simple
- Pipeline: lockstep worker
- Axes: batch:lockstep, worker:multi
- Scenario: An autoscaler needs to know whether to add a replica.
- System must: Export `used/total_slots` (open-slots gauge) per worker as the scale signal (Moshi F9, §9 item 8), updated on every admit/free.
- If mishandled: Without the signal, the autoscaler scales on CPU% (misleading — the GPU is launch-bound) or never scales and saturates.

### BAT-21 — Sample outside the captured graph region
- Level: Simple
- Pipeline: AR TTS lockstep step
- Axes: cuda-graph, batch:lockstep
- Scenario: TTS needs `multinomial` sampling, which is not CUDA-graph-safe; only `argmax` is.
- System must: Capture the deterministic forward in the graph and run the multinomial sampler OUTSIDE the captured region (or use a graph-safe gumbel-argmax inside) — catalog C2, Moshi F7 resolution.
- If mishandled: Graph capture silently breaks sampling, or forces eager and loses the batch-1 edge win.

### BAT-22 — `enforce_eager` as a first-class capture-failure escape
- Level: Simple
- Pipeline: any graphed stage
- Axes: cuda-graph
- Scenario: CUDA-graph capture OOMs or fails on the target (e.g. sm120 capture-OOM after `/health` passes).
- System must: Treat `enforce_eager` as a first-class config knob and auto-downgrade to eager on capture failure — never crash-loop (catalog C8, vLLM H4 #44209).
- If mishandled: The worker enters a capture→OOM→restart crash loop instead of degrading to eager.

---

## INTERMEDIATE — two mechanisms interacting

### BAT-23 — Pipeline overlap: stage N+1 of A ∥ stage N of B
- Level: Intermediate
- Pipeline: AR→codec DAG, 2 streams
- Axes: batch:lockstep, batch:micro
- Scenario: Two streams flow through a 2-stage DAG; the AR thread ticks both while the codec thread micro-batches their frames.
- System must: Run the codec micro-batch for stream A's frame concurrently with the AR step for stream B (temporal interleave on one device; real parallelism if codec is on NPU) — the payoff of decoupled per-stage micro-engines (§3.2).
- If mishandled: Serializing the stages (AR-then-codec per stream) doubles wall time and head-of-line-blocks AR behind the codec.

### BAT-24 — Admission tests the bottleneck stage, not the AR stage
- Level: Intermediate
- Pipeline: AR→CFM→vocoder DAG
- Axes: admission, duty-ledger
- Scenario: AR has free slots but the CFM/vocoder stage is the binding constraint at current load.
- System must: Admit IFF every stage (especially the bottleneck) has free slot + reservable resources + `active < calibrated max`; reject if admitting breaks ANY stage's frame budget (§6, C6).
- If mishandled: AR-only admission seats a stream the codec/CFM can't sustain → the bottleneck stage glitches for everyone.

### BAT-25 — Per-substrate duty ledger: GPU and NPU don't share compute
- Level: Intermediate
- Pipeline: AR (GPU) + codec (NPU) DAG
- Axes: placement, duty-ledger
- Scenario: AR runs on GPU, codec on NPU; both have their own compute budget.
- System must: Keep one compute-duty ledger per substrate and admit IFF `Σ duty(stage on d) ≤ S` for EACH substrate d independently (§6 test (2)).
- If mishandled: Summing GPU+NPU duty into one ledger either over-rejects (treats independent engines as shared) or over-admits one engine.

### BAT-26 — Shared-bandwidth ledger on unified memory (GB10)
- Level: Intermediate
- Pipeline: AR + codec/encoder on GB10 UMA
- Axes: placement, duty-ledger, saturation
- Scenario: GPU, NPU, and CPU stages all draw from the one ~273 GB/s LPDDR ceiling; concurrent engines divide it.
- System must: Maintain a shared bandwidth ledger across substrates on the coherent pool and admit IFF `Σ bandwidth_duty ≤ S·ceiling` (§3.4 contention guard, §6 test (3)).
- If mishandled: Placing codec on NPU "frees the GPU" on paper but oversubscribes the shared bus → both stages slow and the frame budget breaks.

### BAT-27 — Zero-copy handoff across a substrate boundary on coherent memory
- Level: Intermediate
- Pipeline: AR (GPU) → codec (NPU) on GB10
- Axes: placement, zero-copy
- Scenario: The per-frame `TokenFrame` produced on the GPU must be consumed by the codec on the NPU.
- System must: Pass a `ZeroCopyBuffer{ptr, buft, layout, owner, ready_event}` — both substrates advertise `SharedHostBufType`, so the boundary crosses with zero copy (pointer alias), gated by the ready_event (§3.4).
- If mishandled: Inserting a DMA copy at a coherent boundary wastes the shared bandwidth that is the scarce resource, defeating the placement win.

### BAT-28 — Discrete-GPU fallback: async copy of the live slice only
- Level: Intermediate
- Pipeline: AR→codec on a discrete GPU box
- Axes: placement, zero-copy
- Scenario: The same DAG runs on a discrete GPU where the consumer can't view the producer's buffer type.
- System must: Fall back to async copy + event sync + double-buffering, copying only the live `TokenFrame` slice — not the whole tensor (§3.4).
- If mishandled: Copying the full buffer (or syncing the whole device) per frame stalls the pipeline; copying without an event races the consumer.

### BAT-29 — Nested AR-outer + diffusion-inner stays in one forward
- Level: Intermediate
- Pipeline: dots.tts-class `ar_talker{nested cfm}`
- Axes: batch:nested
- Scenario: An AR backbone with a per-frame inner CFM/depth loop (tight feedback).
- System must: Keep the inner loop INSIDE one stage's single batched forward (one `StageNode` with `[stage.nested]`), batching the inner step across all B active slots at the same inner step k (§3.3) — fold `T_inner × inner_steps` into the outer `T_step`.
- If mishandled: Splitting the tight-feedback inner loop into a cross-process stage balloons per-step latency (the dots.tts/qwen3-tts trap both omni engines avoid).

### BAT-30 — Loose-feedback chunk-CFM is a SEPARATE stage
- Level: Intermediate
- Pipeline: CosyVoice2 `ar_semantic → cfm_chunk → vocoder`
- Axes: batch:nested, batch:step-bucket
- Scenario: A talker emits completed semantic chunks that a chunk-CFM consumes (loose feedback).
- System must: Express it as separate DAG nodes (the CFM consumes completed chunks, not per-frame feedback) — fused vs separate is decided by feedback tightness (§3.3).
- If mishandled: Fusing a loose chunk consumer into the AR forward couples a compute-bound 10-step solve to the AR tick and blows the frame budget.

### BAT-31 — Inner per-frame patch batches like AR (38×@64)
- Level: Intermediate
- Pipeline: nested per-frame DiT patch (T4)
- Axes: batch:nested
- Scenario: The nested inner head is a tiny-T latent patch, not a chunk.
- System must: Batch the inner step across the outer lockstep batch — it scales 38×@64 (vs chunk-diffusion's 10×@64) precisely because the tiny latent is launch-bound and under-occupies the GPU alone (§3.3, §1.5).
- If mishandled: Running the inner head per-stream (unbatched) wastes the GPU; treating it like a chunk-CFM mis-budgets its step time.

### BAT-32 — Prefill firewall: ≤1 new stream's prefill per K frames
- Level: Intermediate
- Pipeline: lockstep AR + incoming admissions
- Axes: prefill, admission
- Scenario: Several streams want to start within a few frames; each needs a prefill (style/context embed).
- System must: Admit ≤1 new stream's prefill per K frames and chunk any prefill exceeding one frame-budget's tokens (§4.5) — keep chunk token counts power-of-two (257 is ~32% slower than 256, tile quantization).
- If mishandled: A prefill spike inflates per-token TBT up to 28.3× → 17–22 dropped frames at an 80 ms budget = total dropout for active streams.

### BAT-33 — Force-chunk a long audio-prompt encode
- Level: Intermediate
- Pipeline: token-AR STT / voice-clone prefill
- Axes: prefill
- Scenario: A request carries a long reference-audio prompt whose encode exceeds one frame budget.
- System must: Force-chunk the long audio-prompt encode (a `long_prefill_token_threshold`) so it never monopolizes a tick (vLLM #37308: 147× TTFT head-of-line block).
- If mishandled: The long encode head-of-line-blocks every concurrent stream's frame for the duration of the encode.

### BAT-34 — Non-preemptible: gate admission on whole-stream fit
- Level: Intermediate
- Pipeline: lockstep AR
- Axes: admission
- Scenario: Load is high; a naive scheduler would admit-then-preempt mid-utterance under pressure.
- System must: Gate admission on WHOLE-STREAM fit (not first-chunk) and NEVER preempt mid-utterance; shed by reject-at-admission, not admit-then-evict (vLLM H2 inversion).
- If mishandled: A preempted half-utterance recomputes its entire prefill and re-evicts (thrash) — and a dropped half-utterance is an audible glitch.

### BAT-35 — NaN/Inf logit → reject the frame, don't glitch
- Level: Intermediate
- Pipeline: lockstep AR sampler
- Axes: batch:lockstep
- Scenario: A logit row contains NaN/Inf (an argmax-on-NaN would pick a garbage codec token).
- System must: Run an always-on `logits.isnan().any()` reduction and on hit reject the frame — repeat prev / codec-silence / greedy-resample (vLLM H1, the single most important inversion).
- If mishandled: The sampler argmaxes a NaN row to a garbage code = audible pop with zero error signal.

### BAT-36 — Non-zero watermark computed exactly from fixed slots
- Level: Intermediate
- Pipeline: lockstep AR admission
- Axes: admission, duty-ledger
- Scenario: KV grows by a known per-frame increment across the fixed slots.
- System must: Compute the watermark exactly as `Σ per-slot next-frame KV growth × lookahead` and reserve it before admitting (vLLM H3) — strictly better than a heuristic fraction; reserve the CUDA-graph-pool delta before admission too.
- If mishandled: A watermark of 0 (vLLM default) invites a preempt storm; reserving too little OOMs at request-1 instead of at boot.

### BAT-37 — Multi-worker: one worker per GPU, free-slot scan
- Level: Intermediate
- Pipeline: lockstep AR, 2 GPUs
- Axes: worker:multi, batch:lockstep
- Scenario: Two GPUs each run a worker; a new stream must land on one.
- System must: Route to the worker with a free slot (free-slot scan), keeping each worker GPU-affine with its own slot table, ledger, and graph pool — no cross-GPU lockstep batch.
- If mishandled: Sending a stream to a full worker queues it behind a tick loop while the other GPU idles; a cross-GPU batch is impossible (separate device state).

### BAT-38 — Step-bucket: NFE=1 collapses to feedforward
- Level: Intermediate
- Pipeline: 1-NFE meanflow / IntMeanFlow head
- Axes: batch:step-bucket, variable-nfe
- Scenario: A flow head is configured with NFE=1 (a single solver pass).
- System must: Treat the NFE=1 case as a plain feedforward micro-batch (no solver loop, no CFG-fold if CFG-free) — the step-bucket key must accept N=1 (L15).
- If mishandled: Wrapping a 1-step head in the N-step solver scaffold adds dead loop/scheduler overhead per request.

### BAT-39 — Variable-stride lockstep for a dynamic-FR codec
- Level: Intermediate
- Pipeline: FlexiCodec-class TTS (3–12.5 Hz, data-dependent)
- Axes: variable-stride, batch:cohort
- Scenario: The codec's frame-rate varies per-utterance and per-frame, not known a-priori.
- System must: Generalize lockstep to "advance a model-dependent variable stride" and let the cohort key tolerate unknown-a-priori rates (L6) — the tick advances by the stride the model reports this frame.
- If mishandled: A fixed-rate cohort assumption desyncs the stream from its own variable clock → frame misalignment.

### BAT-40 — Variable inner-NFE: streams at different NFE can't share a tick
- Level: Intermediate
- Pipeline: CALM/VoxCPM-class (per-stream NFE dial)
- Axes: batch:nested, variable-nfe
- Scenario: Two streams in the same outer cohort run their inner solver at NFE 2 and NFE 10 (a per-stream runtime dial).
- System must: Compose two batchers per AR step — the nested batcher runs a variable-NFE inner micro-batch where streams at the same inner step k co-batch and streams that have finished their (shorter) NFE drop out of the inner batch early (L5).
- If mishandled: Forcing all streams to the max NFE wastes compute on the NFE-2 streams; forcing them to share a fixed inner count corrupts the longer one.

### BAT-41 — MTP advances 3 tokens/step but preserves rectangular lockstep
- Level: Intermediate
- Pipeline: FlashTTS-class MTP-3 acoustic AR
- Axes: batch:lockstep, variable-stride
- Scenario: A multi-token-prediction head emits 3 acoustic tokens per step.
- System must: Treat the Depformer/code-predictor as the MTP mechanism (direct-emit) — it preserves the rectangular lockstep batch (unlike draft-spec-decode), advancing the stride by 3 within one tick (L14).
- If mishandled: Bolting EAGLE/Medusa draft-spec-decode on instead destroys the rectangular batch and is a net slowdown on acoustic tokens (L13).

### BAT-42 — Streaming and non-streaming never mix in one micro-batch
- Level: Intermediate
- Pipeline: codec/vocoder micro-batch stage
- Axes: batch:micro
- Scenario: A streaming TTS frame and a one-shot (POST) synthesize land in the codec inbox together.
- System must: Keep streaming and non-streaming requests in separate batches (SGLang G11 micro-batch collector rule), each with its own `max_batch_size/wait_ms/cost`.
- If mishandled: Mixing them couples the one-shot's larger latency to the streaming frame's deadline (or vice-versa) → underrun.

### BAT-43 — Ring-KV wraparound: mask by logical position
- Level: Intermediate
- Pipeline: lockstep AR, long utterance
- Axes: batch:lockstep
- Scenario: A stream's offset exceeds the ring context size; physical slot order ≠ time order.
- System must: Reconstruct per-cell logical position and mask by `pos ≤ my_pos` (causal) AND window AND never-written⇒−1 (Moshi F4, `kv_cache.rs:121-151`) — bake the Kyutai pre/exact/post-wrap test vectors.
- If mishandled: A naive `j ≤ i` causal mask attends to FUTURE tokens in recycled ring cells → corrupted audio after wraparound.

### BAT-44 — Acoustic-delay ring sized max_delay+2, pad the warm-up window
- Level: Intermediate
- Pipeline: multi-codebook AR TTS
- Axes: batch:lockstep
- Scenario: Codebooks have per-codebook acoustic delays; early frames have no real acoustic token yet.
- System must: Size the per-codebook delay ring `max_delay+2` (the +2 = off-by-one guard) and teacher-force codebooks≥1 to PAD before `step < acoustic_delay` (Moshi F8).
- If mishandled: An undersized ring collides the max-delay write with the oldest read; missing the pad-force reads a non-existent acoustic token (garbage/crash).

### BAT-45 — Future-step marker via a step-ordered BinaryHeap
- Level: Intermediate
- Pipeline: batched ASR egress
- Axes: batch:lockstep
- Scenario: "Stream done" must fire at `now + asr_delay_in_tokens + buffered_frames`, not at input-exhaustion.
- System must: Enqueue the marker in a step-ordered `BinaryHeap<Marker>` and terminate on the marker, not on input-exhaustion (Moshi F5); for one-shot-over-streaming, append the audio + marker + trailing silence to flush the delay pipeline.
- If mishandled: Echoing "done" at input-end truncates the transcript (the delayed model hasn't emitted the last words).

### BAT-46 — Multi-trigger slot-free (don't trust one callback)
- Level: Intermediate
- Pipeline: batched ASR/AR, multi-tenant
- Axes: slot-recycle, batch:lockstep
- Scenario: A connection can die via receiver-closed, sender-disconnect, send-error, ping-timeout (20 s), or idle-timeout (120 s).
- System must: Free the slot from INSIDE the step loop on ANY of those triggers (Moshi F9) plus the EoS-after-drain path — never rely solely on a disconnect callback (can be missed).
- If mishandled: A missed disconnect callback leaks the slot forever → the worker's effective capacity bleeds down.

### BAT-47 — Cap every per-stream bookkeeping map
- Level: Intermediate
- Pipeline: long-lived multi-tenant worker
- Axes: worker:multi, slot-recycle
- Scenario: `_closed`/`_aborted`/`_stream_chunk_counters`-style maps grow per request over days of uptime.
- System must: Cap each per-request set/map (e.g. 10000→trim-5000) and purge per-slot entries on slot-free (SGLang G6, ties Moshi F3/F9).
- If mishandled: A long-lived voice server leaks memory until OOM despite no in-flight growth.

### BAT-48 — Same-process fan-out clones the owned container
- Level: Intermediate
- Pipeline: DAG fan-out (1 result → N stages)
- Axes: batch:micro
- Scenario: One stage's owned payload fans out to multiple downstream stages in-process.
- System must: Move `Box<Payload>` across the in-process channel and clone-on-fan-out the owned container, sharing only immutable tensor leaves via `Arc` (SGLang G5) — never `Arc<Mutex<Payload>>` on fan-out.
- If mishandled: A mutation in one downstream branch corrupts the shared payload seen by the others (aliasing bug under concurrency).

### BAT-49 — KV/prefix key fingerprints injected conditioning
- Level: Intermediate
- Pipeline: voice-clone TTS with a prefix/radix cache
- Axes: batch:lockstep
- Scenario: Two requests have identical token-ids but different ref-audio embeds pasted at placeholder positions.
- System must: Make `extra_key = hash(full N-codebook ref sequence)` over EVERY codebook (cb0-only collides); zero-shot → `extra_key=None` so legit prefix-sharing survives (SGLang G1, L1).
- If mishandled: The cache concludes prefixes match and cross-contaminates KV → silent WRONG-VOICE output, only under concurrency.

### BAT-50 — Explicit FINAL frame; cancelled ≠ completed
- Level: Intermediate
- Pipeline: streaming egress
- Axes: back-pressure
- Scenario: A consumer must distinguish done from stalled-producer and barge-in-cancel from completion.
- System must: Send an explicit FINAL/is_done sentinel; "closed without FINAL" = failure; a barge-in cancel sends a DISTINGUISHABLE terminal frame (SGLang G2).
- If mishandled: Inferring done from absence-of-chunks → premature close or hang; cancel looks like completion to the client.

### BAT-51 — Idle-branch blocking, busy-branch hogging
- Level: Intermediate
- Pipeline: co-located AR + encoder stages
- Axes: worker:multi
- Scenario: A scheduler loop co-located with another stage must not starve it.
- System must: Block on `recv_timeout`/Notify on the IDLE branch and hog the core only when busy (SGLang G3, Moshi F6 2 ms sleep) — and prefer stage=process for the hot AR vs a flaky encoder.
- If mishandled: A `loop { try_recv() }` busy-spin starves the co-located stage (the GIL-starvation hazard's Rust analog: CPU/runtime starvation).

### BAT-52 — Relay credit back-pressure: notify-before-wait
- Level: Intermediate
- Pipeline: sidecar↔stage relay
- Axes: back-pressure, worker:multi
- Scenario: A fast producer feeds a slow consumer across the sidecar relay (default credits=2).
- System must: Block the 3rd in-flight `put` until a receiver releases a credit, and send the data-ready CONTROL message BEFORE awaiting transfer completion on any pull/RDMA transport (SGLang G4).
- If mishandled: Without credits the pool overflows/OOMs; wait-then-notify deadlocks on a receiver-initiates-read transport.

### BAT-53 — Dynamic fan-in: compute the expected source set per request
- Level: Intermediate
- Pipeline: thinker→talker→vocoder / STT→translate→TTS DAG
- Axes: batch:micro
- Scenario: A text-only request produces no audio-encoder output, so a fixed `wait_for=[a,b,c]` would block.
- System must: Use `wait_for_fn(req) → expected_sources` to compute the wait set per request and constrain `route_fn` outputs to the statically-declared `next` (SGLang G11).
- If mishandled: A fixed fan-in deadlocks when a conditional branch doesn't fire; an unconstrained route makes the topology unanalyzable.

### BAT-54 — Pre-payload stream arrival is opt-in, else hard-fail
- Level: Intermediate
- Pipeline: AR→vocoder, parallel paths
- Axes: batch:micro, back-pressure
- Scenario: A vocoder receives AR stream-chunks BEFORE its own request payload (no cross-path ordering).
- System must: Allow `can_accept_stream_before_payload` only as explicit opt-in (latch the codec contract from whichever of payload|chunk-meta arrives first, monotone `chunk_id`); otherwise hard-fail (SGLang G11 out-of-order note).
- If mishandled: Silently accepting an early chunk without the payload corrupts the decode contract.

### BAT-55 — Drift response: stop admitting before shedding
- Level: Intermediate
- Pipeline: bottleneck stage at saturation
- Axes: admission, duty-ledger, saturation
- Scenario: The bottleneck stage shows a sustained p99 breach.
- System must: Stop admitting → shed Batch-priority work → only then shed the newest Realtime stream ≤1/tick, with 60 s hysteresis (§6 FR-S3b) — shed is the backstop, admission is the mechanism.
- If mishandled: Dropping frames for everyone (instead of rejecting at the door) violates the SLO for all streams at once.

### BAT-56 — Graceful relegation beats hard reject at 50% overload
- Level: Intermediate
- Pipeline: mixed Realtime + Batch worker
- Axes: admission, saturation
- Scenario: The worker is 50% over capacity.
- System must: Relegate to a degraded queue / quality-brownout rather than reject-everything (Niyama: 8.6% vs 80% SLO violations; BrownoutServe 7% vs 74% at ~5% acc loss) — hard reject only at true saturation (L9).
- If mishandled: Blanket reject-don't-glitch turns recoverable overload into mass 429s the deadline-aware schedulers would have served.

### BAT-57 — Cohorts share the GPU temporally via the duty ledger
- Level: Intermediate
- Pipeline: two same-device cohorts (12.5 Hz + 25 Hz)
- Axes: batch:cohort, duty-ledger
- Scenario: Two frame-rate cohorts of the same family co-reside on one GPU.
- System must: Time-share the GPU between cohorts via the duty ledger (each cohort ticks its own clock), summing both cohorts' duty into the substrate budget — never fuse them into one step (§4.2).
- If mishandled: Admitting both cohorts to 80% duty each oversubscribes the GPU to 160% → both miss their budgets.

### BAT-58 — Promote Inline→Stage-batched when a 2nd stream arrives
- Level: Intermediate
- Pipeline: auto-mode worker
- Axes: admission, worker:multi
- Scenario: A worker running one stream Inline receives a 2nd concurrent stream.
- System must: Lazily promote to Stage-batched-pipelined (spin up queues + tick loop + ledger on demand) — the DAG/stages/nesting are identical across modes, only the executor differs (§8 `mode=auto`).
- If mishandled: Staying Inline serializes the two streams; eagerly running Stage-batched from boot taxes the single-stream case.

### BAT-59 — Capture the largest cohort graph first, share one pool
- Level: Intermediate
- Pipeline: multi-cohort worker
- Axes: cuda-graph, batch:cohort
- Scenario: A worker will serve cohorts of slot-counts 1,2,4,…,N.
- System must: Use a single shared graph pool, capture the LARGEST cohort first, weak-ref outputs, and freeze GC during capture (vLLM H4) — capture exact counts (0 padding).
- If mishandled: Capturing small-first fragments the pool; live GC during capture corrupts the graph.

### BAT-60 — Kernels tier by batch: graph at low-batch, eager at high-batch
- Level: Intermediate
- Pipeline: lockstep AR, edge vs DC
- Axes: cuda-graph, batch:lockstep
- Scenario: The same step runs at batch-1 (edge) and batch-32 (DC).
- System must: Select CUDA-graph at low-batch/edge (1.21× @ batch-1) and eager/compile at high-batch/DC (graph is 0.72× = slower @ batch-32) — §1.3.
- If mishandled: Forcing graphs at batch-32 slows the DC path; forcing eager at batch-1 forfeits the edge latency win.

### BAT-61 — Codec stage is the safe offload / cross-model dedup point
- Level: Intermediate
- Pipeline: AR→codec, shared Mimi/DAC decoder
- Axes: placement, batch:micro
- Scenario: Three different TTS models all terminate in a shared Mimi decoder.
- System must: Offload the terminal codec node to CPU/another EP and dedup it across models (the highest-value cross-model dedup point, §3.2) — it's the one stage safe to move off the AR clock.
- If mishandled: Running a separate codec per model wastes memory and the chance to free the GPU; offloading a tight-feedback stage instead would break the loop.

### BAT-62 — Progress watchdog keyed on last-audio-emitted
- Level: Intermediate
- Pipeline: any live session
- Axes: saturation
- Scenario: A stage is "alive" (servicing its queue) but emits no audio for an active session.
- System must: Track monotonic last-audio-emitted-at per session; an independent watchdog kills/restarts the sidecar if no audio for >N×frame-interval on an active session (vLLM H9) — device+model-aware deadline, not a flat 300 s.
- If mishandled: A zero-forward-progress loop passes every health check while throughput silently goes to zero (vLLM #39863).

### BAT-63 — `max_num_batched_tokens` = num_slots × per-frame-cost, fixed
- Level: Intermediate
- Pipeline: lockstep AR
- Axes: admission, prefill
- Scenario: The fused batch width (decodes + any piggybacked prefill chunk) must be bounded and tile-aligned.
- System must: Fix `max_num_batched_tokens = num_slots × per-frame-cost` (small, ≥ num_slots or startup-crash) and align the fused width to GB10 tiles (vLLM-core note; Bullet 19.4% SM-idle wave-quantization).
- If mishandled: An unbounded fused width lets a piggybacked prefill blow the tick; a non-tile-aligned width wastes ~19% SM on wave quantization.

### BAT-64 — Force SDPA/cuDNN, never FlashInfer on sm120 aarch64
- Level: Intermediate
- Pipeline: lockstep AR attention on GB10
- Axes: cuda-graph
- Scenario: The attention kernel is being selected at load on Blackwell aarch64.
- System must: Route to cuDNN/SDPA and never FlashInfer on sm120 aarch64 (≈2× e2e regression; vLLM-Omni's own Blackwell default) — §2 kernel routing.
- If mishandled: FlashInfer's paged+ragged+plan() overhead roughly halves end-to-end throughput on the target box.

### BAT-65 — Pin a stage to its resident weights (follow the immovable weights)
- Level: Intermediate
- Pipeline: AR (GPU weights) + codec (NPU weights) DAG
- Axes: placement
- Scenario: The placer must decide where each stage runs.
- System must: Follow the immovable load-once weights — AR's 3–6 GB on GPU → AR on GPU; codec's small weights on NPU → codec on NPU (ggml decision order step 3, §3.4); a manual `substrate` pin is never overridden.
- If mishandled: Placing a stage away from its weights forces a multi-GB weight transfer per run (impossible at frame cadence).

### BAT-66 — Calibration measures step-time WITHOUT the profiler
- Level: Intermediate
- Pipeline: any stage, calibration lifecycle
- Axes: duty-ledger
- Scenario: The duty ledger needs `T_step(B_active)` per stage/substrate.
- System must: Measure under synthetic co-load WITHOUT torch-profiler (profiler distorts latency — "Command Buffer Full" is profiler overhead) and exclude the first-request lazy init; persist keyed `sha256 × device × driver × warm-set` (catalog B, §8.3b).
- If mishandled: Calibrating under the profiler or including warm-up over-states step time → the ledger over-rejects.

### BAT-67 — Cycle-safe content sniffer (carry a `seen` set)
- Level: Intermediate
- Pipeline: payload routing / D2H-sync sniffer
- Axes: batch:micro
- Scenario: A sniffer walks a payload graph that may contain cycles to find CPU tensors.
- System must: Carry a `seen` set so the walk terminates (SGLang G10, matching WaaV's prior CRITICAL sniffer false-positive scar).
- If mishandled: A cyclic payload graph infinite-loops the sniffer (or false-positives), stalling the stage.

### BAT-68 — Hybrid KV: radix prefix-cache + ring suffix for cloned voice
- Level: Intermediate
- Pipeline: voice-clone TTS, repeated same voice
- Axes: batch:lockstep
- Scenario: Many requests reuse the same voice/system-prompt prefix (86%+ cache-hit, Fish S2).
- System must: Run a HYBRID KV — radix/paged prefix-cache for the deterministic ref/system prefix + per-slot ring for the per-utterance suffix (L1, the #1 fix) — keyed by the conditioning hash (BAT-49).
- If mishandled: A pure per-slot ring can't share a prefix across slots → recomputes ~86% of cacheable ref-audio/system KV every request.

---

## COMPOUND — three or more mechanisms, realistic co-load

### BAT-69 — 16-stream lockstep + decoupled codec micro-batch
- Level: Compound
- Pipeline: AR→codec DAG, 16 streams
- Axes: batch:lockstep, batch:micro, back-pressure
- Scenario: 16 same-model streams tick the AR lockstep at 12.5 Hz while the codec thread micro-batches the 16 frames/tick at a 2 ms deadline (M2/M3 accept target).
- System must: Tick AR at B=16 (rides the flat-to-64 curve), drain the codec inbox per 2 ms, stream delta samples, and park AR if the codec inbox fills — sustaining RTF<1 within the frame budget.
- If mishandled: Coupling codec to the AR tick, or letting the codec round-robin at batch>1, gaps the audio under 16-way concurrency.

### BAT-70 — Mixed-frame-rate worker: 12.5 Hz + 75 Hz cohorts co-resident
- Level: Compound
- Pipeline: two TTS families on one GPU
- Axes: batch:cohort, duty-ledger, saturation
- Scenario: A 12.5 Hz (80 ms budget) cohort and a 75 Hz (13.3 ms budget) cohort share the GPU; the 75 Hz cohort is near sub-realtime even at batch-1.
- System must: Tick each cohort on its own clock, keep separate graph captures, and admit only while both cohorts' summed duty ≤ S — recognizing the 75 Hz cohort's tiny budget leaves little headroom (§4.4 frame-rate spread is the biggest throughput lever).
- If mishandled: Fusing clocks underruns the fast cohort; ignoring the 75 Hz budget in the ledger admits a stream that can't make its tick.

### BAT-71 — Nested AR+CFM under a duty ledger, B=32
- Level: Compound
- Pipeline: dots.tts-class nested DAG, 32 streams
- Axes: batch:nested, duty-ledger
- Scenario: 32 streams run an AR-outer + per-frame inner-CFM; the inner step batches 2B=64 with CFG across all slots at inner step k.
- System must: Fold `inner_steps × T_inner` into `T_step`, admit on the calibrated nested `T_step(32)`, and run the inner as one batched kernel per outer frame (§3.3) — the nested patch's 38×@64 keeps it schedulable.
- If mishandled: Budgeting only the outer AR time under-counts the nested cost → admits past the frame budget and underruns.

### BAT-72 — Prefill firewall holds the cadence under an admission burst
- Level: Compound
- Pipeline: lockstep AR, 8 concurrent new streams
- Axes: prefill, admission, batch:lockstep
- Scenario: 8 streams (some with long ref-audio prompts) try to start within 3 frames while 24 streams are already live.
- System must: Admit ≤1 prefill per K frames, force-chunk the long prompts (power-of-two chunks), and reject the overflow with a typed 429+Retry-After — protecting the 24 live streams' cadence (§4.5, §6).
- If mishandled: Admitting all 8 prefills at once spikes TBT 28×, dropping 17–22 frames across every live stream = mass dropout.

### BAT-73 — Heterogeneous placement frees GPU bandwidth for 1.3× more AR streams
- Level: Compound
- Pipeline: AR (GPU) + codec/encoder (NPU/CPU) on GB10
- Axes: placement, zero-copy, duty-ledger, saturation
- Scenario: Codec + STT encoder are moved off the GPU to NPU/CPU to make room for more AR streams (M4 accept target).
- System must: Place codec/encoder on their best engine, zero-copy the boundaries, budget the SHARED bandwidth so the split doesn't oversubscribe the 273 GB/s ceiling — netting ≥1.3× more AR streams (§3.4, §6).
- If mishandled: Freeing GPU compute while ignoring the shared bus oversubscribes bandwidth → both AR and codec slow, net loss.

### BAT-74 — Step-bucket CFM solve threatens the frame budget → chunk/lookahead
- Level: Compound
- Pipeline: chunk-CFM TTS, B=64
- Axes: batch:step-bucket, back-pressure
- Scenario: A 10-step CFM solve at B=64 is 110 ms — exceeding a 40 ms frame budget if run per-frame.
- System must: Amortize the chunk-level diffusion over frames (chunked/lookahead with left-context + crossfade), never run the full solve per-frame, and keep the AR stage decoupled so it isn't head-of-line-blocked (§1.5, §4.2).
- If mishandled: Running the 110 ms solve on the frame clock blows the budget by 2.75× → guaranteed underrun.

### BAT-75 — Masked-idle waste at BS32: compact or budget the cost
- Level: Compound
- Pipeline: lockstep AR with heterogeneous residency
- Axes: mask, compaction, duty-ledger
- Scenario: Under barge-in/VAD-gap churn, ~40% of a BS32 batch's rows are masked-idle on a typical tick (L8: 40% padding @BS32).
- System must: Either compact/repack active slots into a smaller captured cohort OR explicitly budget the masked-slot bandwidth/energy cost in the ledger — under heterogeneous residency the masked rows are NOT free.
- If mishandled: Institutionalizing slowest-stream-paces-all wastes ~40% of the batch's bandwidth and ~48% idle-lane energy with no accounting.

### BAT-76 — Compaction must re-capture or re-key the CUDA graph
- Level: Compound
- Pipeline: lockstep AR, compaction enabled
- Axes: compaction, cuda-graph, slot-recycle
- Scenario: Compacting 13 active rows out of 32 slots changes the live batch shape the graph was captured for.
- System must: Repack into the next-smaller pre-captured cohort (e.g. 16) and replay that graph — NOT shrink the live B ad-hoc (which forces a re-capture every compaction, the catastrophic path of Moshi F7).
- If mishandled: Compaction that changes B without a matching captured graph re-captures every tick = worse than the masked waste it tried to fix.

### BAT-77 — KV-length-aware prefill firewall (token-count is the wrong knob)
- Level: Compound
- Pipeline: long-context token-AR STT + live AR
- Axes: prefill, admission
- Scenario: A long-context prefill at token-budget=8 still shows >4× latency variation as context grows (DuetServe Obs.2).
- System must: Switch the firewall control variable to a KV-length-aware PREDICTED-LATENCY budget (a few-feature latency predictor, MAE ~2.5 ms) instead of a flat token count (L10) — and align the fused batch width to tiles.
- If mishandled: A token-count budget under-counts attention cost on long context → the chunk still blows the tick despite "fitting" the token budget.

### BAT-78 — Intra-node spatial P/D vs the chunked-prefill firewall (A/B)
- Level: Compound
- Pipeline: prefill + decode on one GB10
- Axes: prefill, placement, admission
- Scenario: Chunked prefill mixed into the decode batch causes a >8× TBT tail spike (Nexus 250 ms vs 15 ms decode-only).
- System must: A/B intra-node SM-partition P/D (decode on one partition, prefill on another) against the chunked-prefill firewall, choosing the one that protects the frame deadline on GB10 (L4) — reject only cross-node physical P/D.
- If mishandled: Conflating intra-node spatial P/D with cross-node disagg and rejecting both eats the ~8× chunked-prefill TBT spike.

### BAT-79 — Pinned attention-sink + paged escape for long-form
- Level: Compound
- Pipeline: long-form TTS / 10-min STT / many-turn agent
- Axes: batch:lockstep
- Scenario: A session exceeds the ring context (10 min → 30k+ tokens); the ring is silently lossy.
- System must: Pin attention-sink tokens and provide a paged/full-context escape hatch for long-form (L12) — generic LLM-KV eviction methods FAIL on audio (AudioKV).
- If mishandled: The sliding-window ring forgets early context with wraparound instability → drift/incoherence over a long session.

### BAT-80 — Multi-worker autoscale on used/total + warm over-provisioning
- Level: Compound
- Pipeline: fleet of lockstep workers
- Axes: worker:multi, admission, saturation
- Scenario: Traffic bursts; cold-start is 1.7–12.8 s (BLITZSCALE) — far past any frame budget.
- System must: Scale on `used/total_slots`, keep WARM over-provisioned capacity (repurpose warm Batch capacity to Realtime), and NEVER scale-to-zero/cold-start a worker into the live path (L9, F9).
- If mishandled: Scaling reactively from zero stalls the burst for seconds; without the gauge it can't tell when to scale at all.

### BAT-81 — Reliable barge-in cancel jumps every stage's queue in ≤1 tick
- Level: Compound
- Pipeline: full DAG, live S2S
- Axes: back-pressure, slot-recycle
- Scenario: A user barges in; the cancel must reach AR, CFM, codec, and egress reliably.
- System must: Send the cancel over a reliable per-stage ack channel (NOT fire-and-forget PUB/SUB, which drops to late subscribers — SGLang G9), jump every stage's queue, and free the stream's slot/KV/window within ≤1 tick (§6).
- If mishandled: A best-effort abort published before a late stage connects is lost → the cancelled stream keeps speaking over the user.

### BAT-82 — Barge-in cancel produces a distinguishable terminal frame
- Level: Compound
- Pipeline: AR + LLM + TTS S2S
- Axes: back-pressure
- Scenario: A barge-in cancels mid-utterance; the client must not confuse it with a completed turn.
- System must: Emit a terminal frame that is DISTINGUISHABLE from FINAL-on-completion (SGLang G2), and the cancel must also kill the in-flight LLM/AR work, not just the audio.
- If mishandled: Cancel-looks-like-complete corrupts turn-taking; or the audio stops but the LLM keeps generating a dead turn.

### BAT-83 — Three-layer crash detection fails in-flight, doesn't hang
- Level: Compound
- Pipeline: AR (GPU) ∥ encoder (separate process)
- Axes: worker:multi
- Scenario: The encoder process dies mid-batch; the parent must not answer health-200 while throughput goes to zero.
- System must: Run all three crash layers — scheduler-thread handler fails in-flight reqs, background-task done-callbacks, and a process-liveness monitor (5 s) — and fan-out a `dead` flag to every per-request queue so all sessions fail-fast in ~1 s (SGLang G7, vLLM H6).
- If mishandled: A silent task death wedges the stage; the parent reports healthy while every session hangs (vLLM #39863).

### BAT-84 — PDEATHSIG + ordered teardown frees GPU on parent SIGKILL
- Level: Compound
- Pipeline: GPU sidecar worker
- Axes: worker:multi
- Scenario: The parent is SIGKILLed while the sidecar is inside a CUDA kernel.
- System must: Set `PR_SET_PDEATHSIG(SIGTERM)` at worker entry (kernel-guaranteed even under SIGKILL) and teardown via abort-collectives-before-destroy → SIGTERM→5 s→SIGTERM→4 s→SIGKILL, short-drain-then-abort (vLLM H7) — never hard-cut mid-utterance, never unbounded drain.
- If mishandled: An orphaned GPU sidecar pins VRAM into the next process (vLLM #34643); a death-pipe Event alone fails (a thread in a CUDA kernel never polls it).

### BAT-85 — Wall-clock aging promotes a starved stream
- Level: Compound
- Pipeline: mixed-priority multi-stream worker
- Axes: admission, worker:multi
- Scenario: Under sustained load a low-priority stream waits indefinitely (vLLM has no aging anywhere).
- System must: Promote after `max_wait` with FCFS-within-slot-pool + hard per-slot fairness; if priority is used, the comparison key MUST include an age/preemption term (vLLM H8, the #41951 omission).
- If mishandled: A low-priority stream is never admitted under load (priority starvation), or a re-admitted victim loses its place because the key ignores wait time.

### BAT-86 — Streaming delta-only with byte-identical offline==stream test
- Level: Compound
- Pipeline: streaming TTS egress
- Axes: batch:micro, back-pressure
- Scenario: The egress must yield only NEW samples per step, not re-decode from step 0.
- System must: Stream delta-only (O(N) not O(N²)), audit the emit→consolidate→consume chain (the consolidation path must concat the audio key, not skip it), and gate on `offline_concat == stream_concat` byte-for-byte (catalog I1/C1).
- If mishandled: Cumulative re-decode is O(N²) and users hear replays/truncation — "the MOST COMMON silent bug" (offline RTF still passes).

### BAT-87 — Zero D2H syncs in the per-frame loop (the 2400-syncs trap)
- Level: Compound
- Pipeline: Path-B torch sidecar, AR decode
- Axes: batch:lockstep, bs1
- Scenario: A naive sidecar calls `.item()/.cpu()/.tolist()` per step (10 steps × 60 frames × 4 ops = 2400 syncs/request).
- System must: Keep the per-frame loop sync-free (`dst.copy_(src)` not `fill_(src.item())`, `torch.where`/masking not Python branches, `torch.compile(forward, fullgraph=False)`) and gate on a zero-D2H-sync test during decode (catalog I3/C5).
- If mishandled: Each sync is a GPU→CPU stall → latency collapse that erases the clean 9 ms/step the whole budget assumes.

### BAT-88 — Path-B sidecar keys codec/window state by slot, frees on reset
- Level: Compound
- Pipeline: Path-B torch sidecar, concurrent streams
- Axes: batch:lockstep, slot-recycle
- Scenario: The sidecar holds Python codec buffers / sliding-window pads / streaming-generator state across `forward()`.
- System must: Key all sidecar state by slot-id (`self._state[slot]`) and free on slot-reset, with a concurrent-crosstalk test (catalog I5/C3) — the lockstep multi-session step verb extends the IPC protocol with slot-id + per-slot input.
- If mishandled: A shared buffer crosstalks audio across concurrent requests — symptom is crosstalk/truncation only under load.

### BAT-89 — Capability-driven CUDA-graph ladder auto-downgrades on sm120
- Level: Compound
- Pipeline: AR lockstep + varlen encode/EoT on GB10
- Axes: cuda-graph, batch:lockstep
- Scenario: AR decode is FULL-graph-safe but the audio-encode/varlen/EoT/sampling paths are not; sm120 has graph-hang scars.
- System must: Tag each kernel `AttentionCGSupport{ALWAYS..NEVER}`, resolve the MIN across groups, FULL-graph the AR decode (capture once/cohort), run encode/varlen eager-or-piecewise, and auto-downgrade to eager on any unsupported path — never crash (vLLM H4).
- If mishandled: Forcing a FULL graph over a varlen path silently corrupts (vLLM #45425), or hangs after N requests on sm120 (#40969).

### BAT-90 — `dst.zero_()` padded slots; never write −1 into a real KV slot
- Level: Compound
- Pipeline: lockstep AR with any padding
- Axes: cuda-graph, mask, slot-recycle
- Scenario: A captured cohort is larger than the live slot count, so some slots are padding.
- System must: `dst.zero_()` the padded slots (and mask them) so a pad write never lands in a real KV slot (vLLM #43810 wrote −1 into a real slot) — combined with the exact-count capture (BAT-12) this is rare, but defended.
- If mishandled: A pad-fill of −1 into an occupied KV slot silently corrupts that stream's attention.

### BAT-91 — Control-plane / data-plane separation, ref-held until send
- Level: Compound
- Pipeline: sidecar IPC + streaming egress
- Axes: back-pressure, worker:multi
- Scenario: Small msgpack control messages and raw PCM data both cross the sidecar/egress boundary.
- System must: Separate control-plane (small msgpack) from data-plane (raw PCM, zero-copy, ref-held until the send completes) so a control message never waits behind a PCM blob and the PCM buffer isn't freed mid-send (vLLM-core note).
- If mishandled: Interleaving them stalls control latency (barge-in!) behind audio; freeing the PCM ref early sends garbage.

### BAT-92 — Per-stream determinism only (bitwise cross-stream is impossible)
- Level: Compound
- Pipeline: lockstep AR batch
- Axes: batch:lockstep
- Scenario: A request must be reproducible, but atomic reductions make cross-stream bitwise determinism impossible (vLLM #24067).
- System must: Accept per-stream-only determinism, seed the sampler per slot (float64 Gumbel), and pass a seeded `generator` to any CFM/diffusion step so CFG-parallel doesn't diverge from sequential (catalog B) — reproducibility is per-stream, keyed by slot RNG state gated through the mask.
- If mishandled: Expecting batch-wide bitwise determinism is unmet; an unseeded CFG-parallel step is non-deterministic vs the sequential reference.

### BAT-93 — Calibration-gated admission rejects rather than glitches at saturation
- Level: Compound
- Pipeline: full DAG worker at the knee
- Axes: admission, duty-ledger, saturation
- Scenario: The worker reaches its calibrated capacity (M4 accept: admission rejects rather than glitches).
- System must: At saturation, reject new streams with a typed 429/503 + Retry-After (per-substrate + bottleneck + shared-bandwidth tests all consulted) and keep every admitted stream glitch-free (§6) — NEVER admit-and-degrade (P-4).
- If mishandled: Admit-and-degrade glitches the whole worker; rejecting the wrong (non-bottleneck) signal either over- or under-admits.

### BAT-94 — Slow consumer: bounded drop-oldest, not unbounded accumulation
- Level: Compound
- Pipeline: streaming TTS to a slow client
- Axes: back-pressure
- Scenario: A TTS consumer reads slower than the engine produces (HWM=0 would accumulate GBs of stale audio).
- System must: Use a bounded per-stream egress buffer; the ordered queue stays unbounded ONLY with sender-side credit back-pressure (never drop/reorder audio), otherwise drop-oldest the worthless stale tail (vLLM H2, SGLang G6).
- If mishandled: Unbounded HWM=0 silently accumulates stale audio to OOM; dropping mid-stream reorders or glitches.

### BAT-95 — Migration drops ≥1 frame unless the playback buffer masks it
- Level: Compound
- Pipeline: DC spill/rebalance across replicas
- Axes: worker:multi, saturation
- Scenario: A stream is migrated between replicas (KV transfer is sub-ms–5 ms) but one decode-step > one frame.
- System must: Mask the migration gap with the CLIENT playback buffer (VoxServe/TokenFlow cadence protection) — append-only constant-time KV migration, and only migrate when the playback buffer has slack (L16, §6 Llumnix-style).
- If mishandled: Mid-stream migration drops ≥1 frame audibly because the migration step exceeds one frame period.

### BAT-96 — Binary streaming-viability objective + risk-of-violation scheduling
- Level: Compound
- Pipeline: many-stream worker (VoxServe-style)
- Axes: admission, saturation, duty-ledger
- Scenario: Once a frame will deliver in time, further latency reduction is worthless; the scheduler should prioritize streams by risk-of-deadline-violation.
- System must: Adopt the binary deliver-in-time objective and a soft-deadline scheduler that prioritizes by risk-of-violation (slack-driven), not by minimizing already-safe latency (L3 VoxServe) — protected by the client playback buffer.
- If mishandled: Optimizing mean latency over deadline-risk starves the at-risk streams (the ones about to underrun) to speed up already-safe ones.

### BAT-97 — Sniffer + readiness + warmup interlock on cold boot
- Level: Compound
- Pipeline: worker boot → first request
- Axes: cuda-graph, admission
- Scenario: A worker boots, must capture graphs, calibrate the ledger, and only then serve.
- System must: Pre-capture feasibility-check at boot (fail at boot, not request-1), run warm-up off the hot path, calibrate without the profiler, and gate `/readyz` on warmup+calibration complete (vLLM H3/H4, F6, C7) — readiness returns non-200 until all three finish.
- If mishandled: `/readyz` 200 before capture/calibration admits a request into an uncaptured/un-budgeted engine → first-request cliff or capture-OOM crash-loop.

### BAT-98 — Co-located encoder must pass a starvation load test
- Level: Compound
- Pipeline: AR + STT encoder co-located on one GPU
- Axes: worker:multi, batch:micro
- Scenario: Colocating the hot AR loop with an encoder forward saves a process but risks interference (SGLang moved to encoder DISAGGREGATION because of it).
- System must: Prefer stage=process for hot-AR vs encoder; if co-located, the idle branch must block (BAT-51) AND the colocation must pass a starvation load-test before it's allowed (SGLang G3 deeper lesson).
- If mishandled: A busy AR loop slows the co-located encoder ~600× (audio QPS >10→<0.5) under load.

### BAT-99 — TTS accuracy gate = offline-parity + streaming-playback + concurrent-load
- Level: Compound
- Pipeline: model-readiness gate for a batched TTS
- Axes: batch:micro, batch:lockstep
- Scenario: A new/quantized TTS passes offline RTF/WER but hasn't been tested streaming or concurrent.
- System must: Run all three validation layers — offline parity (with a perceptual/MOS check, not WER-only), browser streaming playback (catches delta/cumulative + chunk-boundary + TTFP), and concurrent `max_num_seqs>1` with 4+ parallel (catches per-slot state leaks + codec-window round-robin gaps) — before it serves (catalog I4/C4, §5.2).
- If mishandled: A WER-flat/MOS-crash AR-drift quant ships; or a per-slot crosstalk bug surfaces only in production under load.

### BAT-100 — Pipelined-single mode: queues + watchdog, no cross-request batch
- Level: Compound
- Pipeline: 1-to-handful streams, one model
- Axes: worker:multi, back-pressure
- Scenario: A worker serves a handful of streams of one model with pipeline overlap but B_max=1 per stage.
- System must: Run per-stage threads + bounded queues for pipeline overlap, a light memory-ledger + watchdog only, and NO cross-request batching (§8 Pipelined-single) — promote to Stage-batched only when many streams / multi-model arrives.
- If mishandled: Running the full per-substrate duty ledger + cross-request batcher for a handful of streams adds overhead the mode doesn't need.

### BAT-101 — Fan-in merge for text+audio S2S (multi-terminal, narrowed)
- Level: Compound
- Pipeline: thinker→{text sink, talker→vocoder} S2S
- Axes: batch:micro, back-pressure
- Scenario: A turn produces text only, or text+audio; the merge must handle both terminals.
- System must: Collect partials by stage, gate on the per-request expected set, ignore inactive terminals, and let the request narrow its terminals (text-only vs text+audio) — `route_fn` outputs ∈ static `next`, empty forbidden (SGLang G11).
- If mishandled: A fixed multi-terminal merge hangs waiting for the audio terminal on a text-only turn.

### BAT-102 — Variable-residency churn: admit/free/recycle every few ticks
- Level: Compound
- Pipeline: live S2S with frequent barge-in/EOS
- Axes: slot-recycle, batch:lockstep, mask
- Scenario: Turn-taking churns slots — admit, drain-and-free, recycle — multiple times within a short window across 32 slots.
- System must: On each tick apply control-plane (admits/frees/resets) FIRST, then compute exec-mask, then run the kernel only if `any()` (Moshi F6); each recycle is transactional (BAT-10) with a channel-id bump (BAT-11) and tail-drain-before-free (BAT-17).
- If mishandled: Interleaving admits with the kernel mid-tick, or a non-transactional recycle, corrupts a slot or contaminates the new occupant.

### BAT-103 — Decoupled stage sizes survive a codec slowdown without AR HoL
- Level: Compound
- Pipeline: AR(≥4)→codec(1) DAG under load
- Axes: batch:lockstep, batch:micro, back-pressure
- Scenario: The codec stage slows (e.g. a placement contention spike); AR must not be head-of-line-blocked by it (M3 accept: codec no longer HoL-blocks AR).
- System must: Keep AR batching independently of codec batch size, park AR only on a genuinely full codec inbox, and let the codec's own micro-batch recover — never collapse the two stages' sizes (§3.2, C6).
- If mishandled: A shared batch size makes the codec slowdown stall AR for every stream (audio gaps), the exact RFC #2568 bug.

### BAT-104 — Sub-300 ms first-audio across a 3-node DAG under concurrency
- Level: Compound
- Pipeline: CosyVoice2 3-node DAG, multiple streams
- Axes: batch:lockstep, batch:step-bucket, batch:micro
- Scenario: M3 accept: a 3-node `ar_semantic→cfm_chunk→vocoder` DAG must stream first-audio sub-300 ms while several streams run.
- System must: TTFA-ramp the first codec chunk (larger-for-quality then smaller-for-latency), pipeline-overlap the three stages, and budget `first_audio = frame_period + acoustic_delay·frame_period + step_time` (§4.4) — keep the CFM amortized (BAT-74).
- If mishandled: A cold first chunk or a per-frame CFM solve pushes first-audio past 300 ms (perceived as a slow/laggy assistant).

---

## EXTREME — the saturated heterogeneous worker

### BAT-105 — 64 lockstep AR + 8 step-bucket CFM + micro-batch codec + STT encoder, one worker
- Level: Extreme
- Pipeline: full heterogeneous DAG on one GB10 worker
- Axes: batch:lockstep, batch:step-bucket, batch:micro, batch:nested, batch:cohort, placement, duty-ledger, saturation, worker:multi
- Scenario: A single worker runs 64 lockstep AR streams (12.5 Hz) + 8 step-bucket CFM streams + a micro-batched shared codec stage + an STT encoder, mixed frame-rates, some streams idling/resuming, one slot recycled mid-tick, under a per-substrate duty ledger near saturation.
- System must: Per tick — apply control-plane (the mid-tick recycle: transactional reset + channel-id bump) FIRST; substitute masked/warming rows to BOS and gate every per-slot mutation through the mask; run the AR lockstep step (exact-count graph) and fan its frames into the codec micro-batch (2 ms) while the 8-stream CFM step-buckets (CFG-folded) on its own clock; place codec/encoder off-GPU with zero-copy + shared-bandwidth budget; admit nothing that breaks ANY substrate/bottleneck/bandwidth budget; meter per-step wall time vs each cohort's frame budget; if the bottleneck p99 breaches, stop admitting → shed Batch → shed newest Realtime ≤1/tick.
- If mishandled: ANY of — an ungated masked row NaNs the whole 64-batch; the mid-tick recycle contaminates the new occupant; the CFM solve HoL-blocks AR; the shared bus oversubscribes; an AR-only admission overruns the codec — and the worker glitches every one of 72+ live streams at once.

### BAT-106 — Slot recycled mid-tick while the kernel is mid-flight
- Level: Extreme
- Pipeline: 64-stream lockstep AR
- Axes: slot-recycle, batch:lockstep, mask
- Scenario: A disconnect+new-admit for slot 41 lands while the current tick's kernel is already submitted for the full batch.
- System must: Defer the recycle to the control-plane phase of the NEXT tick (control-plane runs before the kernel, never during) — the in-flight kernel completes for the old occupant, output is dropped by the stale channel-id, then reset_slot + new prefill happen before the next step (Moshi F6 ordering + F3 channel-id).
- If mishandled: Resetting KV/word-buffers under a mid-flight kernel races the read → CUDA illegal-memory or the new user inherits the old user's in-flight frame.

### BAT-107 — Saturation cascade: bottleneck shifts from AR to CFM to bandwidth
- Level: Extreme
- Pipeline: AR + nested CFM + codec on GB10 UMA
- Axes: duty-ledger, saturation, placement, batch:nested
- Scenario: As streams ramp, the binding constraint migrates — first AR compute, then the CFM solve, then the shared LPDDR bandwidth ceiling.
- System must: Keep all three ledgers live (per-substrate compute ×N + shared bandwidth) and re-evaluate the bottleneck on every admission so the admit/shed decision always tests the CURRENT binding stage/resource (§6) — the bottleneck is not statically the AR stage.
- If mishandled: Admitting against a stale (AR-only) bottleneck oversubscribes whichever resource actually became binding → underrun for all.

### BAT-108 — Mixed-NFE inner solve across a churning outer batch
- Level: Extreme
- Pipeline: nested AR-outer + per-stream variable-NFE inner (CALM/FlashTTS-class)
- Axes: batch:nested, variable-nfe, mask, slot-recycle
- Scenario: 32 outer slots, each running its inner solver at a per-stream NFE (2–10); slots idle/resume and one recycles, all within the nested forward.
- System must: At outer frame t, batch only the slots at the same inner step k (variable-NFE drop-out as shorter-NFE streams finish), gate the inner-loop per-slot state through the exec-mask, and fold the worst-case `max(inner_steps)×T_inner` into the admission budget (L5 nested-batcher composes two batchers per step).
- If mishandled: Forcing a uniform inner NFE corrupts the short-NFE streams or wastes compute on them; an ungated inner-state mutation corrupts a resumed slot's solve.

### BAT-109 — Dynamic-FR cohort + fixed-FR cohort + nested cohort, one ledger
- Level: Extreme
- Pipeline: FlexiCodec (3–12.5 Hz) + Mimi (12.5 Hz) + nested-dots.tts on one worker
- Axes: batch:cohort, variable-stride, batch:nested, duty-ledger, saturation
- Scenario: Three cohorts with incompatible clocks (one data-dependent variable-stride, one fixed 12.5 Hz, one nested) time-share the GPU near saturation.
- System must: Tick each cohort on its own (possibly variable) stride with its own graph capture, sum all three cohorts' duty into the GPU ledger, and admit per cohort against the remaining budget — never lockstep-mix the clocks, tolerate the variable-stride cohort's unknown-a-priori rate (L6, §4.2).
- If mishandled: Treating the variable-stride cohort as fixed-rate desyncs it; summing the wrong duty oversubscribes the GPU across the three incompatible clocks.

### BAT-110 — Hybrid-KV worker: radix prefix-cache shared across 64 cloned-voice slots
- Level: Extreme
- Pipeline: 64-stream voice-clone TTS, repeated voices
- Axes: batch:lockstep, saturation, duty-ledger
- Scenario: 64 streams, many reusing a handful of cloned voices (86%+ prefix-cache hit), each with a per-utterance ring suffix, near saturation.
- System must: Serve the deterministic ref/system prefix from a shared radix/paged cache (keyed by the conditioning hash, `None`-escape for genuine matches) while each slot owns its ring suffix — and budget the prefix-cache memory + the suffix rings together in admission (L1 hybrid KV + BAT-49 keying).
- If mishandled: A pure per-slot ring recomputes ~86% of cacheable ref KV ×64 (forfeiting the dominant commercial workload's efficiency); a mis-keyed prefix cross-contaminates voices across slots.

### BAT-111 — Graph re-capture storm avoided under churn + compaction + cohort shifts
- Level: Extreme
- Pipeline: 64-slot lockstep with compaction enabled, heavy churn
- Axes: cuda-graph, compaction, slot-recycle, mask, saturation
- Scenario: Heavy admit/free churn would shift the active-slot count constantly; compaction wants to repack; both threaten per-tick re-capture.
- System must: Mask-not-drop idle slots (one graph per pre-captured exact-count cohort), and on compaction repack into the NEXT-smaller pre-captured cohort — so the live shape always matches a captured graph and re-capture never happens on the hot path (Moshi F7 + BAT-76).
- If mishandled: Any path that changes B without a matching captured graph re-captures every frame = catastrophic (seconds of capture stalling the live batch).

### BAT-112 — Prefill firewall + spatial-P/D + 64 live AR + long voice-clone prompt
- Level: Extreme
- Pipeline: 64 live AR + an incoming long-ref-audio clone request on GB10
- Axes: prefill, admission, placement, batch:lockstep, saturation
- Scenario: A new voice-clone request with a long ref-audio prompt arrives while 64 AR streams are live near saturation; its encode exceeds many frame budgets.
- System must: Run the encode on a separate SM partition (intra-node spatial P/D) OR force-chunk it (power-of-two, KV-length-aware predicted-latency budget), admit ≤1 prefill/K frames, and reject if it breaks the 64 streams' cadence — protecting the live cohort's frame deadline (L4, L10, §4.5).
- If mishandled: Mixing the long encode into the decode batch spikes TBT >8× → all 64 live streams drop frames during the encode.

### BAT-113 — Multi-worker fleet: rebalance, crash-isolate, autoscale under burst
- Level: Extreme
- Pipeline: fleet of GB10/B200 workers, mixed cohorts
- Axes: worker:multi, saturation, admission
- Scenario: A traffic burst hits a fleet; one worker's sidecar crashes; another nears saturation; the autoscaler must add warm capacity.
- System must: Route by free-slot scan + `used/total` signal, fail-fast every in-flight stream on the crashed worker via the 3-layer detection + dead-flag fan-out, migrate movable streams only with playback-buffer slack (constant-time KV migration), and repurpose WARM Batch capacity rather than cold-start into the live path (SGLang G7, vLLM H6, L9, L16).
- If mishandled: A crashed worker hangs its sessions; reactive cold-start stalls the burst seconds; migration without playback slack drops frames fleet-wide.

### BAT-114 — Per-stage SLO decomposition holds end-to-end TTFA under full co-load
- Level: Extreme
- Pipeline: full DAG, near-saturation, mixed priority
- Axes: admission, duty-ledger, saturation, batch:lockstep, batch:step-bucket, batch:micro
- Scenario: The session TTFA p90 ≤ budget + streaming-viability ≥99.9% must hold while AR, CFM, codec, and STT stages all carry their own SLO near saturation.
- System must: Decompose the session SLO into per-stage budgets `T_step(stage,B) ≤ S·(1000/sps)`, give every stage its own duty entry + SLO, admit on the bottleneck, piggyback Batch into leftover budget (Sarathi), and shed in priority order on a sustained breach (§6) — the binding stage (often CFM/codec, not AR) governs.
- If mishandled: Budgeting only end-to-end (or only the AR stage) lets the real bottleneck stage silently miss its budget → streaming viability falls below 99.9% under co-load.

---

## Coverage

This catalog covers the **batching / scheduling / worker / stage-DAG** family across four tiers (Simple BAT-1–22, Intermediate BAT-23–68, Compound BAT-69–104, Extreme BAT-105–114):

- **Lockstep fixed-slot batching** — free-slot admission (BAT-1, BAT-37), all-idle short-sleep (BAT-2), exec-mask / masked≠absent input-substitution (BAT-14), gate-every-mutation (BAT-15), idle-then-resume byte-identity (BAT-16), transactional slot recycling (BAT-10, BAT-106), channel-id stale-output guard (BAT-11), ring-KV wraparound logical-position mask (BAT-43), acoustic-delay ring (BAT-44), future-step marker (BAT-45), tail-drain-before-free (BAT-17), multi-trigger slot-free (BAT-46).
- **Cohort-by-(model,frame-rate)** — clock separation (BAT-5), temporal time-share (BAT-57), mixed-FR co-residence (BAT-70), variable-stride dynamic-FR codecs (BAT-39), MTP variable stride (BAT-41), three-cohort saturation (BAT-109).
- **Step-bucket batcher** — length/CFG bucketing (BAT-7), NFE=1 collapse (BAT-38), eroding bucket key / per-request N (BAT-38, L15), CFM-budget amortization (BAT-74).
- **Micro-batch collector** — 2 ms codec deadline (BAT-6), decoupled AR≥4/codec=1 sizes (BAT-8, BAT-103), streaming≠non-streaming (BAT-42), pre-payload arrival (BAT-54), shared-codec dedup (BAT-61).
- **Nested batcher (two batchers per AR step)** — in-forward tight feedback (BAT-29), loose-feedback separate stage (BAT-30), per-frame patch 38×@64 (BAT-31), variable inner-NFE (BAT-40, BAT-108), nested under ledger (BAT-71).
- **Stage-DAG pipeline parallelism** — overlap N+1∥N (BAT-23), back-pressure parking (BAT-9), bottleneck admission (BAT-24), dynamic fan-in (BAT-53), multi-terminal merge (BAT-101), no-AR-HoL under codec slowdown (BAT-103), sub-300 ms 3-node first-audio (BAT-104).
- **Heterogeneous placement + zero-copy + bandwidth-duty** — per-substrate ledger (BAT-25), shared-bandwidth ledger (BAT-26, BAT-107), zero-copy coherent handoff (BAT-27), discrete-GPU fallback (BAT-28), follow-the-weights placement (BAT-65), placement frees 1.3× streams (BAT-73), no-FlashInfer routing (BAT-64).
- **Multi-worker** — worker-per-GPU free-slot scan (BAT-37), used/total autoscale (BAT-20, BAT-80), warm over-provisioning (BAT-80), crash isolation 3-layer (BAT-83), PDEATHSIG teardown (BAT-84), migration playback-mask (BAT-95), fleet rebalance (BAT-113).
- **CUDA-graph per exact-slot-count cohort** — lifetime graph (BAT-12), warm-up off hot path (BAT-13), sample-outside-graph (BAT-21), enforce_eager escape (BAT-22), largest-first shared pool (BAT-59), batch-tiered kernels (BAT-60), capability-ladder auto-downgrade (BAT-89), zero padded slots (BAT-90), re-capture-storm avoidance (BAT-111).
- **bs=1 fast-path** — skip overlap (BAT-4), zero-D2H-sync loop (BAT-87), inline mode (BAT-19).
- **Masked-idle-slot waste** — 40%@BS32 compaction/budget (BAT-75), compaction re-key (BAT-76).
- **Prefill firewall** — ≤1/K-frames + power-of-two chunk (BAT-32, BAT-72), force-chunk long encode (BAT-33), KV-length-aware predicted-latency budget (BAT-77), intra-node spatial P/D A/B (BAT-78, BAT-112).
- **Non-preemptible whole-stream-fit admission** — whole-stream gate (BAT-34), exact watermark (BAT-36), max_num_batched_tokens fixed (BAT-63), reject-not-glitch at saturation (BAT-93), bounded drop-oldest (BAT-94).
- **Scheduler correctness / overload** — NaN-reject-frame (BAT-35), drift response (BAT-55), graceful relegation (BAT-56), wall-clock aging (BAT-85), progress watchdog (BAT-62), binary viability + risk scheduling (BAT-96), per-stage SLO decomposition (BAT-114).
- **Streaming / IPC correctness** — delta-only test (BAT-86), explicit FINAL / cancel≠complete (BAT-50, BAT-82), reliable barge-in (BAT-81), slot-keyed sidecar state (BAT-88), control/data-plane split (BAT-91), credit back-pressure notify-before-wait (BAT-52), fan-out clone (BAT-48), cap bookkeeping (BAT-47), cycle-safe sniffer (BAT-67), per-stream determinism (BAT-92), conditioning-hash KV key (BAT-49), hybrid prefix-cache (BAT-68, BAT-110).
- **Mode/lifecycle** — inline (BAT-19), pipelined-single (BAT-100), auto-promote (BAT-58), readiness interlock (BAT-97), calibration-without-profiler (BAT-66), starvation load-test (BAT-98), idle-branch-block (BAT-51), 3-layer validation gate (BAT-99).
- **Long-form / hybrid-KV** — pinned attention-sink + paged escape (BAT-79).
- **EXTREME saturated worker** — the full 64-AR + 8-CFM + codec + STT mixed-FR churn under near-saturation ledger (BAT-105), plus its decomposed extreme corners (BAT-106–114).

**Total: 114 distinct scenarios.**
