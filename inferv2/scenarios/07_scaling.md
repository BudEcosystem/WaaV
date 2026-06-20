# 07 — Scaling / Multi-tenancy / Deployment / Lifecycle Scenarios

Real-world scaling situations for WaaV Infer: 1 stream on an edge GB10 → thousands of concurrent streams across an autoscaling B200 fleet, one binary config-tiered (Inline → Pipelined-single → Stage-batched, §8), with the per-substrate duty ledger + bottleneck admission (§6), keep-alive co-residency, multi-tenant isolation, the Loading→Warming→Ready⇄Degraded→Draining→Failed lifecycle FSM, canary/blue-green rollout, and KV-migration spill. Grounded in INFER_ENGINE.md §6/§8 and the production-failure catalog (slot-leak F3/F9, crash blast-radius G7/H6, cold-start L9, prefix-salt G1, KV-migration L16, never-scale-to-zero L9).

Each scenario is **distinct**: no duplicates, no padding. Levels: **SIMPLE** (single mechanism), **INTERMEDIATE** (two mechanisms interact), **COMPOUND** (three-plus, realistic ops), **EXTREME** (worst-case flash-crowd / rolling-upgrade / fault storms).

---

## SIMPLE

### SCALE-1 — Single edge stream, Inline mode, no machinery
- Level: SIMPLE
- Pipeline: any AR-TTS or STT
- Axes: scale:edge, worker:single, tier:inline
- Scenario: One phone/edge call hits a fresh GB10 (273 GB/s, latency-only). `mode=auto`, no co-tenant.
- System must: Run all stages inline on the calling thread in DAG order (B=1), no queues, no tick loop, no admission, no ledger; first-audio bounded by `frame_period + acoustic_delay + step_time`.
- If mishandled: Edge pays DC tax (tick loop, ledger threads) → needless latency/jitter on a single stream.

### SCALE-2 — Second concurrent stream triggers auto-promotion
- Level: SIMPLE
- Pipeline: AR-TTS
- Axes: scale:autoscale, worker:single, tier:auto-promote
- Scenario: A box running Inline (1 stream) receives a 2nd concurrent stream on the same model.
- System must: Lazily promote Inline → Pipelined-single (then Stage-batched if a 3rd arrives), spinning up per-stage queues + the ledger on demand; the in-flight stream is NOT interrupted (same stage-forward, executor swap only).
- If mishandled: Promotion stalls/restarts the first stream → audible glitch, or both streams contend on one inline thread → both miss frame budget.

### SCALE-3 — `mode=edge` pin refuses promotion
- Level: SIMPLE
- Pipeline: any
- Axes: scale:edge, worker:single, tier:pinned
- Scenario: Operator pins `mode=edge` on a single-purpose appliance; a 2nd stream arrives anyway.
- System must: Stay Inline and REJECT the 2nd stream with a typed 429/503 + Retry-After (the box is provisioned for 1); never silently promote past the pinned tier.
- If mishandled: Honoring the pin but admitting → two streams serialize on the inline thread → both underrun.

### SCALE-4 — `mode=dc` pin forces Stage-batched at boot
- Level: SIMPLE
- Pipeline: any
- Axes: scale:DC, worker:multi, tier:pinned
- Scenario: A DC replica starts with `mode=dc` even though only 1 stream is live initially.
- System must: Start Stage-batched (full §6 ledger, decoupled micro-engines) immediately; a single stream rides B=1 through the batched executor without waiting for a cohort.
- If mishandled: bs=1 takes a batch-coalesce path that waits for a 2nd stream → first stream eats the `max_batch_wait_ms` deadline for no reason (G8: async/overlap is net-negative at bs=1).

### SCALE-5 — used/total_slots emitted for autoscaler
- Level: SIMPLE
- Pipeline: any (lockstep)
- Axes: scale:autoscale, worker:multi, signal:slots
- Scenario: An external autoscaler polls each replica's load.
- System must: Export `used/total_slots` per (model, frame-rate) cohort (the Kyutai §9.8 signal) plus per-substrate duty %, as the canonical scale-up/down signal; cheap, lock-free gauge read.
- If mishandled: Scaling on CPU% or QPS (not slot occupancy) → scale lags the real frame-budget pressure → admission rejects before the autoscaler reacts.

### SCALE-6 — Model cold-load on first deploy
- Level: SIMPLE
- Pipeline: any
- Axes: lifecycle:cold-start, worker:single, fsm:loading
- Scenario: A replica boots and must load a 3 GB AR codec-LM from disk to GPU.
- System must: Enter `Loading`, stream weights to device, then `Warming`; `/readyz` returns 503 until warm+calibrated (C7). Cold-start is 1.7–12.8 s (L9) — expected, gated, not on the request path.
- If mishandled: `/readyz` returns 200 on process-up → traffic routed mid-load → first requests hit a half-loaded model (NaN/crash) or the cold-start cliff inline.

### SCALE-7 — Warmup gates readiness (first-request cliff)
- Level: SIMPLE
- Pipeline: AR + CUDA-graph
- Axes: lifecycle:warmup, fsm:warming, kernel:cuda-graph
- Scenario: Model loaded but CUDA-graph not yet captured; first request would pay seconds of capture inline.
- System must: Run warmup 2–3 full-mask steps + `synchronize()` (F6) to force graph capture OFF the hot path; only then flip `Warming → Ready` and serve. Readiness predicate = warm+calibration complete.
- If mishandled: First real request triggers lazy capture → first caller eats multi-second TTFA (#44209: capture OOMs AFTER /health passes).

### SCALE-8 — Admin load of a second model onto an idle device
- Level: SIMPLE
- Pipeline: two models co-resident
- Axes: deploy:admin-load, co-residency:multi-model, worker:single
- Scenario: Admin issues load(model B) on a box already serving model A, with headroom.
- System must: Load B into spare VRAM, register its cohort, calibrate B (sha×device×driver), flip B to Ready; A keeps serving uninterrupted the whole time.
- If mishandled: Load pauses A's tick loop (shared lock) → A's live streams underrun during B's load.

### SCALE-9 — Admin unload of an idle co-resident model
- Level: SIMPLE
- Pipeline: two models → one
- Axes: deploy:admin-unload, co-residency:multi-model
- Scenario: Admin unloads model B (zero live slots) to reclaim VRAM for A's growth.
- System must: Confirm B has zero active slots, free B's weights + KV arenas + graph pool, update the duty ledger; refuse unload (or drain first) if B still has live streams.
- If mishandled: Unload with live slots → use-after-free of B's KV → crash takes the whole process (G7 blast-radius).

### SCALE-10 — Keep-alive TTL expiry evicts an idle model
- Level: SIMPLE
- Pipeline: multi-model co-residency
- Axes: co-residency:keep-alive, lifecycle:ttl
- Scenario: Model C served its last stream 11 minutes ago; TTL is 10 min; VRAM is tight.
- System must: On TTL expiry with zero live slots, evict C (LRU/TTL), free its resources, mark it cold; next request for C pays a documented reload (warm-pool miss).
- If mishandled: Never evicting → VRAM fills with cold models → the next hot model's load OOMs.

### SCALE-11 — Slot freed on clean client disconnect
- Level: SIMPLE
- Pipeline: lockstep
- Axes: lifecycle:slot-free, worker:multi
- Scenario: A streaming caller hangs up cleanly mid-stream.
- System must: Free the slot from INSIDE the step loop on receiver-closed (F9), transactionally reset KV/conv-rings/sampler/word-buffers (F3), decrement used_slots; the slot is immediately reusable.
- If mishandled: Relying only on a disconnect callback (can be missed) → slot leaks → effective capacity erodes over hours (F9).

### SCALE-12 — Drain on graceful shutdown
- Level: SIMPLE
- Pipeline: any
- Axes: deploy:drain, fsm:draining
- Scenario: An operator sends SIGTERM to a replica for a planned restart.
- System must: Enter `Draining` → `/readyz` 503 (stop new admissions), let in-flight utterances finish, then exit; short-drain-then-abort (never hard-cut mid-utterance, never unbounded drain) (H7).
- If mishandled: Hard-cut on SIGTERM → every live call truncated mid-word; or unbounded drain → deploy hangs forever on one stuck stream.

### SCALE-13 — Reject at slot saturation (reject-don't-glitch)
- Level: SIMPLE
- Pipeline: lockstep
- Axes: scale:DC, admission:reject, worker:multi
- Scenario: All N slots on a cohort are occupied; a new stream arrives.
- System must: REJECT with typed 429/503 + Retry-After (H2: never preempt mid-utterance, never admit-and-degrade); the autoscaler reads the rejection + slot gauge and adds capacity.
- If mishandled: Admit-then-evict → a running half-utterance is dropped to make room → audible glitch for the victim (the vLLM preempt-thrash class).

### SCALE-14 — Warm over-provisioning, never scale-to-zero
- Level: SIMPLE
- Pipeline: any
- Axes: scale:autoscale, lifecycle:warm-pool
- Scenario: Traffic to a model drops to near-zero overnight.
- System must: Keep ≥1 warm replica (warm over-provisioning), NEVER scale-to-zero (cold-start 1.7–12.8 s would put the next caller on the cliff) (L9); idle warm capacity is the cost of realtime SLO.
- If mishandled: Scale-to-zero to save cost → the 3 a.m. caller waits 12.8 s for first audio (dead air) before the model even loads.

### SCALE-15 — Heterogeneous fleet: route by substrate capability
- Level: SIMPLE
- Pipeline: AR-TTS + conv STT
- Axes: fleet:mixed-gpu, worker:multi, placement:capability
- Scenario: A fleet has GB10 edge boxes and B200 DC nodes; a 1-stream edge request and a 500-stream batch both arrive.
- System must: Route the 1-stream latency job to GB10/edge (bf16 + CUDA-graph, latency-only) and the big-batch job to B200 (fp8/mxfp4, push toward compute-bound); the substrate sets the ceiling, not the design (§8 one binary).
- If mishandled: 1 stream on B200 = severe under-occupancy (wasted); 500 streams on GB10 = blow the 273 GB/s shared ceiling.

### SCALE-16 — Per-tenant prefix-cache salt at admission
- Level: SIMPLE
- Pipeline: token-AR STT / cloned-voice TTS with prefix cache
- Axes: multi-tenancy:isolation, security:salt
- Scenario: Two tenants send identical text/system-prompt prefixes to a shared prefix-cache replica.
- System must: Salt the prefix-cache key per-tenant (`cache_salt` on block-0, sha256 NEVER xxhash) AND fingerprint injected conditioning (G1: blake2b over all codebooks) so tenant A never reads tenant B's cached KV.
- If mishandled: Shared prefix hash → cross-tenant KV leak (privacy disaster) or wrong-voice output under concurrency (G1).

### SCALE-17 — Per-tenant slot quota
- Level: SIMPLE
- Pipeline: lockstep
- Axes: multi-tenancy:quota, worker:multi
- Scenario: One tenant tries to open 60 of a 64-slot cohort's slots.
- System must: Enforce a per-tenant slot quota at admission (e.g. ≤ fair-share); reject the tenant's overflow with 429 while leaving slots for others.
- If mishandled: One tenant monopolizes the cohort → every other tenant is starved (noisy-neighbor at the slot level).

### SCALE-18 — Calibration stamp is sha×device×driver keyed
- Level: SIMPLE
- Pipeline: any
- Axes: lifecycle:calibration, fleet:mixed-gpu
- Scenario: The same model sha is loaded on a GB10 and on an H200 with different driver versions.
- System must: Calibrate `T_step(B_active)` per stage **per (sha256 × device × driver × warm-set)** (§6) and persist; a GB10 stamp is never reused for the H200 admission math.
- If mishandled: Reusing GB10 timings on B200 (8 TB/s) → admission math wrong → over- or under-admit → frame-budget misses or wasted capacity.

### SCALE-19 — Frame-budget admission test on a single stage
- Level: SIMPLE
- Pipeline: AR
- Axes: capacity:frame-budget, admission:reject
- Scenario: A cohort is at B=30; the AR step at B=31 is calibrated to push duty past S=0.8.
- System must: Reject the 31st stream because admitting breaks `Σ duty(stage) ≤ S` on the AR substrate (§6 condition 2); the slot count alone says room, the duty says no.
- If mishandled: Admit on slot-count-only → the AR step overruns the frame period → ALL 31 streams underrun (isochronous violation).

### SCALE-20 — Single-binary config-scaling, no code fork
- Level: SIMPLE
- Pipeline: any
- Axes: deploy:one-binary, scale:edge↔DC
- Scenario: The same artifact is deployed to a phone, a GB10, and a B200 rack.
- System must: Select tier + batch-ceiling + precision tier from config + observed load (§8); the DAG, stages, nested loops, and placement hints are IDENTICAL — only the executor and batch ceiling differ.
- If mishandled: Maintaining separate edge/DC builds → drift between them → a bug fixed in DC silently persists on edge.

---

## INTERMEDIATE

### SCALE-21 — Load-while-serving OOM avoided by pre-admission reservation
- Level: INTERMEDIATE
- Pipeline: model A serving + model B loading
- Axes: co-residency:multi-model, lifecycle:oom-guard
- Scenario: Admin loads model B while A is at high slot occupancy; B's weights + graph-pool + B's first KV arenas would exceed remaining VRAM.
- System must: Reserve B's full footprint (weights + CUDA-graph-pool delta + min KV) BEFORE committing the load (H3 pre-capture feasibility); if it won't fit alongside A's live reservation, refuse the load with a typed error — don't start then OOM.
- If mishandled: Optimistic load → OOM mid-capture → crash takes A's live streams too (G7).

### SCALE-22 — Version hot-swap with zero-downtime cohort handoff
- Level: INTERMEDIATE
- Pipeline: AR-TTS v1 → v2
- Axes: deploy:hot-swap, lifecycle:fsm, co-residency:multi-version
- Scenario: Model A-v2 must replace A-v1 on a live replica without dropping calls.
- System must: Load v2 alongside v1 (both Ready), route NEW streams to v2, let v1's in-flight streams drain to completion, then unload v1; never migrate a stream across model versions mid-utterance.
- If mishandled: Atomic swap → v1's live streams lose their KV (different shapes/weights) → mass truncation.

### SCALE-23 — Canary: 5% of new streams to the new model
- Level: INTERMEDIATE
- Pipeline: STT v-new
- Axes: deploy:canary, multi-tenancy:fairness
- Scenario: A new STT checkpoint is canaried to 5% of new sessions across the fleet.
- System must: Route 5% of NEW admissions to the canary cohort (sticky per session for its lifetime), keep the canary's own accuracy/duty gate, and auto-rollback the canary if its `waav_quant_gate_failed` / drift breaches fire — without touching the 95% on stable.
- If mishandled: Canary shares slots with stable on one cohort → a canary regression's frame-misses bleed into stable's SLO.

### SCALE-24 — Blue-green replica swap behind the LB
- Level: INTERMEDIATE
- Pipeline: any
- Axes: deploy:blue-green, fsm:draining
- Scenario: Green replicas (new build) come up warm; blue (old) must retire.
- System must: Bring green to Ready+warm+calibrated, shift the LB to green for new streams, drain blue (short-drain-then-abort), retire blue only after its last utterance finishes; readiness-gated cutover (no green traffic until `/readyz` 200).
- If mishandled: Cut over before green warms → first green callers hit the cold-start cliff fleet-wide (thundering herd onto unwarmed replicas).

### SCALE-25 — Burst handling within a warm replica's headroom
- Level: INTERMEDIATE
- Pipeline: lockstep
- Axes: scale:autoscale, burst:absorb
- Scenario: A 20-call burst lands on a replica with 18 free slots before the autoscaler can react.
- System must: Admit 18 into free slots (frame-budget permitting), reject 2 with Retry-After; the warm over-provisioning headroom absorbs the burst's leading edge while new capacity spins up (bursts ~2.3 s to absorb, L9).
- If mishandled: No headroom (provisioned at exactly steady-state) → the burst's leading edge is all rejections → caller-visible failures every traffic spike.

### SCALE-26 — Warm-capacity repurposing across cohorts
- Level: INTERMEDIATE
- Pipeline: model A (idle) + model B (hot)
- Axes: scale:autoscale, co-residency:repurpose
- Scenario: Model A's warm replicas are idle; model B is saturating; both fit the same GPU class.
- System must: Repurpose A's warm capacity to B — unload A on a spare replica (zero live slots), load B, calibrate, add to B's cohort — rather than cold-spinning a brand-new node (faster than cold-start).
- If mishandled: Leave A idle + cold-spin new B nodes → B callers wait the full cold-start while warm silicon sits unused.

### SCALE-27 — Noisy-neighbor isolation via MPS partition
- Level: INTERMEDIATE
- Pipeline: two tenants, two models, one GPU
- Axes: multi-tenancy:isolation, gpu:mps
- Scenario: Two tenants co-reside on one B200 via CUDA MPS; tenant A's batch job spikes compute.
- System must: Bound each tenant's SM share (MPS active-thread-percentage) so A's spike can't starve B's realtime streams; the per-substrate duty ledger accounts each tenant's stages separately.
- If mishandled: Unpartitioned MPS → A's compute spike steals SMs → B's AR step overruns the frame budget → B's calls glitch (priority inversion across tenants).

### SCALE-28 — MIG hard-partition for a premium tenant
- Level: INTERMEDIATE
- Pipeline: per-tenant model
- Axes: multi-tenancy:isolation, gpu:mig
- Scenario: A premium tenant requires guaranteed isolation (no shared-bandwidth interference).
- System must: Place the premium tenant on a dedicated MIG slice (hard memory+SM partition) with its own duty ledger; other tenants share the remaining slices via MPS; the placer respects the MIG topology.
- If mishandled: Treating a MIG slice as full-GPU bandwidth in admission math → over-admit the slice → the "isolated" tenant still underruns.

### SCALE-29 — One bad request must not stall the cohort
- Level: INTERMEDIATE
- Pipeline: lockstep
- Axes: multi-tenancy:fairness, fault:one-bad-request
- Scenario: One stream sends a pathological prefill (huge style embedding) that would inflate the step time for the whole batch.
- System must: Chunk/cap that stream's prefill to ≤1 frame-budget of tokens (prefill firewall §4.5; admit ≤1 prefill per K frames), keep its tokens power-of-two-aligned; the other 63 slots keep ticking on budget.
- If mishandled: Run the giant prefill in-batch → per-token TBT inflates up to 28.3× → 17–22 dropped frames for EVERYONE in the cohort (total dropout).

### SCALE-30 — NaN logit from one stream rejected, batch survives
- Level: INTERMEDIATE
- Pipeline: lockstep
- Axes: multi-tenancy:fault-isolation, numerics:nan
- Scenario: One slot's logits go NaN (bad audio / quant edge case) inside a 64-wide batched step.
- System must: Run an always-on `logits.isnan().any()` reduction (H1 inversion) and reject-that-frame for the offending slot (repeat-prev / codec-silence), substitute a valid token so the dense kernel stays well-formed (F1); the other 63 slots emit correctly.
- If mishandled: Argmax a NaN row → garbage codec token (audible pop) for that user, OR an unsubstituted masked row → CUDA illegal-memory that kills ALL 64 (F1).

### SCALE-31 — Idle-then-resume slot has no cross-talk
- Level: INTERMEDIATE
- Pipeline: lockstep
- Axes: multi-tenancy:isolation, fault:idle-resume
- Scenario: Slot 7 goes idle (user silent / barge-in pause) for 200 frames, then resumes, while 30 other slots tick.
- System must: Gate EVERY per-slot mutation through `where(exec_mask, new, old)` (F2: offset, KV scatter, conv ring, RoPE phase, sampler RNG) so the masked slot's state is frozen, not corrupted; on resume it's byte-identical to a never-idled stream.
- If mishandled: One ungated mutation → RoPE phase jump / poisoned ring cells → slot 7's transcript silently corrupts on resume (invisible in single-stream tests, F2).

### SCALE-32 — Slot recycling across tenants is transactional + scrubbed
- Level: INTERMEDIATE
- Pipeline: lockstep
- Axes: multi-tenancy:isolation, security:slot-recycle
- Scenario: Tenant A's stream in slot 7 disconnects; tenant B is immediately admitted into slot 7.
- System must: One transactional `reset_slot(7)` fanning out to KV pointers + conv rings + sampler + word buffers + offset (F3); a monotonic `channel_id` guard drops any of A's late output/markers; B's `positions/indices=0` make A's stale KV bytes unreachable.
- If mishandled: No reset → B's attention reads A's KV + word buffer → cross-tenant transcript contamination (privacy disaster, F3).

### SCALE-33 — Drift response: stop admitting, then shed
- Level: INTERMEDIATE
- Pipeline: AR + codec
- Axes: scale:DC, overload:drift, admission:shed
- Scenario: Sustained p99 breach on the BOTTLENECK stage (codec, not AR) under creeping load.
- System must: Stop admitting first → shed Batch-priority work → only then shed newest Realtime ≤1/tick, with 60 s hysteresis (FR-S3b); admission is the mechanism, shed is the backstop.
- If mishandled: Shedding before stopping admission → churn (shed one, admit two) → oscillation; or admitting on AR-duty while the codec is the binding constraint (the AR-only-admission bug §6).

### SCALE-34 — Bottleneck-stage admission (codec binds, not AR)
- Level: INTERMEDIATE
- Pipeline: 3-node CosyVoice2 (ar → cfm → vocoder)
- Axes: capacity:bottleneck, admission:per-stage
- Scenario: AR has free slots and headroom, but the chunk-CFM stage (10×@64, 110 ms solve) is near its frame budget.
- System must: Test admission against the BOTTLENECK stage's duty (§6: ∀ stage), reject if the CFM/vocoder can't sustain another stream even though AR can; per-stage independent batch sizes (AR≥4, codec=1, C6).
- If mishandled: Admit on AR headroom → the CFM stage round-robins its window across too many requests → audio gaps under concurrency (RFC #2568, C6).

### SCALE-35 — Calibration under synthetic co-load, not in isolation
- Level: INTERMEDIATE
- Pipeline: multi-stage
- Axes: lifecycle:calibration, capacity:duty
- Scenario: A new model's `T_step` is measured at boot to seed the admission ledger.
- System must: Calibrate `T_step(B_active)` under synthetic CO-LOAD (other stages running) — not isolated — because the shared substrate's contention is real; measure WITHOUT a profiler (profiler distorts latency, catalog §B); exclude the first-request lazy init.
- If mishandled: Calibrate in isolation → optimistic timings → admission over-admits under real co-load → frame-budget misses appear only in production.

### SCALE-36 — Intra-node spill before cross-node
- Level: INTERMEDIATE
- Pipeline: lockstep
- Axes: scale:DC, spill:intra-node
- Scenario: One GPU in an 8-GPU node saturates while a sibling GPU has free slots.
- System must: Spill the next stream to the sibling GPU on the SAME node first (intra-node, cheapest) before considering a cross-node replica; the placer prefers boundary minimization.
- If mishandled: Cross-node spill while an idle sibling sits one NVLink hop away → needless network KV transfer + worse tail.

### SCALE-37 — KV-migration spill masked by playback buffer
- Level: INTERMEDIATE
- Pipeline: AR
- Axes: scale:DC, spill:kv-migration, mask:playback-buffer
- Scenario: A live stream must move from a draining replica to a fresh one mid-utterance.
- System must: Use append-only KV migration (sub-ms–5 ms for voice ctx, L16); since one decode step > one frame, the unavoidable ≥1-frame gap is MASKED by the client playback buffer (VoxServe-style cadence protection), not heard.
- If mishandled: Migrate without playback-buffer slack → the migration's frame gap is an audible underrun (L16: mid-stream migration drops ≥1 frame unless masked).

### SCALE-38 — Heterogeneous placement frees GPU bandwidth on GB10
- Level: INTERMEDIATE
- Pipeline: AR (GPU) + conv codec/encoder (NPU/CPU)
- Axes: scale:edge, placement:heterogeneous, substrate:unified
- Scenario: A GB10 serving AR streams is bandwidth-bound on the shared 273 GB/s LPDDR.
- System must: Place the codec/encoder stage on the NPU/idle-SMs/CPU (zero-copy via `SharedHostBufType`) to free GPU bandwidth for ≥1.3× more AR streams (M4 accept); admission budgets the SHARED bandwidth so the split doesn't oversubscribe the one ceiling (§3.4 contention guard).
- If mishandled: Offload the codec but ignore shared-bandwidth budgeting → both engines divide the 273 GB/s ceiling → net fewer streams, not more.

### SCALE-39 — Shared-bandwidth ledger on unified memory
- Level: INTERMEDIATE
- Pipeline: AR + codec co-resident on GB10
- Axes: capacity:shared-bandwidth, substrate:unified
- Scenario: Both the AR (memory-bound) and a second model's codec stage saturate the GB10 LPDDR bus.
- System must: Run a shared-bandwidth ledger (§6 condition 3): `Σ bandwidth_duty ≤ S·ceiling` across ALL stages on the coherent pool; prefer to overlap a memory-bound stage with a compute-bound one, co-locate+time-share when both saturate bandwidth.
- If mishandled: Per-substrate compute ledger only (no bandwidth ledger) → two memory-bound stages admitted independently → the bus is oversubscribed → both stall.

### SCALE-40 — Crash blast-radius contained to one process group
- Level: INTERMEDIATE
- Pipeline: AR (GPU process) + flaky encoder (separate process)
- Axes: fault:crash-isolation, deploy:process-topology
- Scenario: The encoder stage segfaults on a malformed input.
- System must: Run the hot AR stage and the flaky encoder as SEPARATE processes (G3/G7) so the encoder crash fails only its in-flight requests (3-layer crash detection: scheduler handler + done-callbacks + liveness monitor); the AR cohort keeps serving.
- If mishandled: Colocate AR+encoder in one process → encoder crash exits the whole group → every AR stream on the box dies (G7 blast-radius).

### SCALE-41 — Dead sidecar → failed requests, not a hang
- Level: INTERMEDIATE
- Pipeline: Path-B torch sidecar
- Axes: fault:sidecar-death, fsm:failed
- Scenario: The torch sidecar process is SIGKILLed (OOM-killer) mid-serving.
- System must: Detect via death-sentinel byte + out-of-band waitpid/pidfd watcher (H6); a single `dead` flag drives BOTH new-admission-reject AND propagate-error fan-out into every per-request queue → all sessions fail-fast in ~1 s, the replica flips to `Failed`/`Degraded`.
- If mishandled: Parent answers /readyz 200 while the sidecar is dead and throughput is 0 (#39863) → traffic keeps routing to a black hole (callers hang).

### SCALE-42 — Orphaned GPU sidecar can't pin VRAM
- Level: INTERMEDIATE
- Pipeline: Path-B torch sidecar
- Axes: fault:orphan, lifecycle:teardown
- Scenario: The parent server is SIGKILLed; the GPU sidecar is mid-CUDA-kernel (can't poll a death-pipe).
- System must: Set `PR_SET_PDEATHSIG=SIGTERM` at sidecar entry (kernel-guaranteed even under SIGKILL, H7) so the orphan dies and releases VRAM; teardown order aborts collectives before destroy.
- If mishandled: Orphaned sidecar pins VRAM into the next process (#34643) → the replacement replica OOMs on load.

### SCALE-43 — Progress watchdog keyed on last-audio-emitted
- Level: INTERMEDIATE
- Pipeline: AR
- Axes: fault:stall, lifecycle:watchdog
- Scenario: A stage is "alive but zero forward progress" (deadlocked but the loop spins) — passes every health check.
- System must: Track monotonic "last-audio-emitted-at T" per session; an independent thread kills/restarts the sidecar if an active session emits no audio for >N×frame-interval (H9); the deadline is device+model-aware (a 1.5B AR-TTS step ≠ a CTC step, NOT a flat 300 s).
- If mishandled: Liveness-only health passes → a wedged stage holds slots forever → silent capacity loss + dead-air for its sessions (#39863 blind spot).

### SCALE-44 — Per-tenant fairness under FCFS slot pool
- Level: INTERMEDIATE
- Pipeline: lockstep
- Axes: multi-tenancy:fairness, admission:aging
- Scenario: A heavy tenant continuously offers load; a light tenant's occasional stream waits.
- System must: FCFS-within-slot-pool + hard per-slot fairness + wall-clock AGING (H8): promote a waiting stream after `max_wait` so the light tenant is never starved forever (vLLM lacks aging — the #41951 omission).
- If mishandled: No aging → the heavy tenant's continuous arrivals always win the next freed slot → the light tenant's stream waits indefinitely.

### SCALE-45 — Calibration cache hit on warm restart
- Level: INTERMEDIATE
- Pipeline: any
- Axes: lifecycle:calibration, deploy:restart
- Scenario: A replica restarts on the SAME sha×device×driver it ran before.
- System must: Load the persisted `verified{substrate,precision,metric}` + `T_step` stamps (cheap stamp-check, §5.2/§6) instead of re-running full calibration → faster Ready; re-calibrate only on any key change.
- If mishandled: Re-calibrate from scratch every restart → multi-second extra cold-start → slower fleet rollouts and worse burst recovery.

### SCALE-46 — Degraded state on accelerator fault, not Err
- Level: INTERMEDIATE
- Pipeline: AR + codec
- Axes: fault:accelerator, fsm:degraded
- Scenario: The NPU hosting the codec stage faults; the codec can still run degraded-to-CPU.
- System must: Degrade-to-CPU + emit telemetry (P-6: an accelerator problem is a degrade event, never an `Err`); flip the replica `Ready → Degraded` (lower slot ceiling on the now-CPU codec), keep serving at reduced capacity, drain toward repair.
- If mishandled: Treat the NPU fault as fatal → kill the replica → unnecessary capacity loss when a degraded path existed.

### SCALE-47 — Cohort split by frame-rate on a multi-rate box
- Level: INTERMEDIATE
- Pipeline: 12.5 Hz Mimi-TTS + 25 Hz STT
- Axes: capacity:cohort, scale:DC
- Scenario: One replica serves two models at different frame-rates.
- System must: Keep separate (model, frame-rate) cohorts — never lockstep-mix a 12.5 Hz and a 25 Hz clock (§4.2); cohorts share the GPU TEMPORALLY via the duty ledger, each with its own slot table + admission.
- If mishandled: One fused batch across frame-rates → no common realtime tick → both cohorts desync (the slower paces the faster, or the faster starves).

### SCALE-48 — Lazy ledger spin-up on co-tenant load
- Level: INTERMEDIATE
- Pipeline: model A (Pipelined-single) + model B loads
- Axes: tier:auto-promote, co-residency:multi-model
- Scenario: A box in Pipelined-single (1 model, few streams) gets a 2nd MODEL loaded.
- System must: Promote to Stage-batched and spin up the per-substrate duty ledger on demand (§8 `mode=auto`) because two co-resident models now share the substrate; A's streams keep flowing through the same stage-forward.
- If mishandled: Stay Pipelined-single with no ledger → A and B's stages contend unaccounted → both miss budget under the shared substrate.

### SCALE-49 — Power-of-two prefill chunk alignment
- Level: INTERMEDIATE
- Pipeline: prefill firewall
- Axes: capacity:prefill, kernel:tile-align
- Scenario: A stream's prefill is 257 tokens; the kernel tile is 256.
- System must: Chunk prefill keeping counts power-of-two-aligned (§4.5: 257 is ~32% slower than 256 — tile quantization); align the fused batch width (chunk + piggybacked decodes) to GB10 tiles.
- If mishandled: 257-token chunk → tile-quantization wave → the prefill chunk overruns its frame budget → cadence break for the cohort.

### SCALE-50 — Reliable barge-in cancel across all stages
- Level: INTERMEDIATE
- Pipeline: STT → translate → TTS DAG
- Axes: multi-tenancy:fairness, control:barge-in
- Scenario: A user barges in; the in-flight multi-stage utterance for that session must cancel.
- System must: A reliable abort channel with per-stage ack (G9: fire-and-forget PUB/SUB DROPS to late subscribers) jumps every stage's queue and frees that stream's slot/KV/window within ≤1 tick; cancelled ≠ completed (distinct terminal frame, G2).
- If mishandled: Best-effort abort lost to a late stage → the cancelled utterance keeps generating → holds a slot + talks over the user (barge-in cannot be best-effort).

---

## COMPOUND

### SCALE-51 — Rolling model upgrade across a fleet, no dropped calls
- Level: COMPOUND
- Pipeline: AR-TTS v1 → v2 fleet-wide
- Axes: deploy:rolling, fsm:draining, scale:DC, worker:multi
- Scenario: 40 replicas must move v1→v2 one-at-a-time under steady traffic.
- System must: For each replica: stop admitting (→Draining), drain in-flight to completion, load+warm+calibrate v2, flip back to Ready; the LB shifts load to the remaining warm replicas during each drain; keep enough warm headroom that draining one never rejects (capacity planning the rollout).
- If mishandled: Drain too many at once → the shrunken warm pool rejects bursts; or atomic per-replica swap → v1 streams truncated.

### SCALE-52 — Canary regression auto-rollback under live SLO
- Level: COMPOUND
- Pipeline: STT v-new canary
- Axes: deploy:canary, lifecycle:gate, overload:drift
- Scenario: The 5% canary's bottleneck-stage p99 starts breaching + its accuracy stamp fails on a fixture re-check.
- System must: Fire `waav_quant_gate_failed` / drift, auto-rollback the canary cohort (route its NEW streams back to stable, drain canary streams), keep the 95% stable untouched, alert; the canary's failure never touched stable's slots (isolated cohort).
- If mishandled: No auto-rollback → the canary's regression accumulates failed sessions; or shared cohort → stable inherits the canary's frame-misses.

### SCALE-53 — Flash crowd onto autoscaling fleet, warm headroom + spill
- Level: COMPOUND
- Pipeline: AR-TTS
- Axes: scale:autoscale, burst:flash-crowd, spill:intra-node, worker:multi
- Scenario: 800 concurrent calls arrive in 3 s against a fleet sized for 500 steady.
- System must: Absorb the leading edge into warm headroom + free slots, spill intra-node first, reject the true overflow with Retry-After (reject-don't-glitch), and signal the autoscaler via slot/duty gauges to add replicas (warm-repurpose first, cold-spin last); existing 500 streams keep their cadence.
- If mishandled: Admit-and-degrade to "fit" 800 → all 800 (incl the original 500) underrun; or no headroom → the entire leading edge is rejections.

### SCALE-54 — Multi-model co-residency + keep-alive churn under mixed traffic
- Level: COMPOUND
- Pipeline: 6 model variants on one B200
- Axes: co-residency:multi-model, lifecycle:ttl, capacity:duty
- Scenario: 6 variants share a B200; traffic shifts hourly so the hot set rotates; VRAM fits ~4 hot at once.
- System must: Keep-alive TTL/LRU evicts the 2 coldest (zero live slots), loads the newly-hot on demand with pre-admission reservation (no load-while-serving OOM), recalibrates each on load, and the per-variant duty ledgers keep the shared substrate budgeted; evictions never touch a model with live slots.
- If mishandled: Thrash (evict→reload→evict) on borderline traffic, or load-without-reservation OOM crashes the box taking all 6 down.

### SCALE-55 — Noisy-neighbor batch tenant vs realtime tenant on shared GPU
- Level: COMPOUND
- Pipeline: tenant-A realtime AR + tenant-B batch STT
- Axes: multi-tenancy:isolation, gpu:mps, capacity:priority
- Scenario: Tenant B runs a huge batch-transcription job on the same GPU as tenant A's live calls.
- System must: Priority `Realtime > Batch` per stage (Sarathi piggyback of B into A's leftover budget), MPS SM-bound B, account both in the duty ledger; B's throughput flexes to whatever A leaves, A's frame budget is protected.
- If mishandled: B's batch saturates the GPU unbudgeted → A's AR step overruns → A's paying realtime calls glitch while B's offline job runs fast (priority inversion).

### SCALE-56 — Edge↔DC config-scaling validated on the same artifact
- Level: COMPOUND
- Pipeline: AR-TTS
- Axes: deploy:one-binary, scale:edge↔DC, kernel:tiered, precision:tiered
- Scenario: The same signed artifact must run N=1 on GB10 (bf16 + CUDA-graph) and N=400 on B200 (fp8/mxfp4, eager/compile).
- System must: Select CUDA-graph @ low-batch/edge and eager/compile @ high-batch/DC (§1.3: graphs hurt 0.72× @ batch-32), bf16 @ batch-1 vs fp8 @ large-M (§1.6: fp8 0.62× @ M=64 but 2.1× @ M=4096), per-substrate calibration each side; identical DAG, only ceiling+precision+kernel-tier differ.
- If mishandled: One global kernel/precision choice → CUDA-graph slows the B200 batch, or fp8 slows the GB10 single stream (wrong lever at the wrong scale).

### SCALE-57 — Per-tenant SLO tiers (gold/silver) under contention
- Level: COMPOUND
- Pipeline: lockstep
- Axes: multi-tenancy:slo, capacity:priority, admission:reject
- Scenario: Gold tenants have a tighter TTFA SLO than silver; the box approaches saturation.
- System must: Reserve slot/duty quota per SLO tier, admit gold preferentially (risk-of-violation scheduling, VoxServe L3), shed silver first under drift; each tier's admission tests its own bottleneck-stage budget; no tier is starved to zero (aging).
- If mishandled: Flat FCFS → silver bursts crowd out gold → gold's premium SLO breaches; or gold starves silver to zero (no fairness floor).

### SCALE-58 — Calibration mismatch on a driver upgrade mid-fleet
- Level: COMPOUND
- Pipeline: any
- Axes: lifecycle:calibration, fleet:mixed-driver, deploy:rolling
- Scenario: Half the fleet is upgraded to a new CUDA driver; the same sha now has different `T_step`.
- System must: Key calibration by sha×device×DRIVER×warm-set (§6); upgraded replicas re-calibrate (their stamp key changed) before serving, old replicas keep their valid stamp; admission math per replica uses its OWN stamp — never a fleet-wide average.
- If mishandled: Share one calibration across driver versions → the upgraded replicas over/under-admit → frame misses appear only on the upgraded half (hard to diagnose).

### SCALE-59 — Long-form session outlives a replica drain
- Level: COMPOUND
- Pipeline: long-form TTS (audiobook)
- Axes: lifecycle:long-session, deploy:drain, spill:kv-migration
- Scenario: A 20-minute TTS session is live on a replica scheduled for drain; its context is long (ring would be lossy, L12).
- System must: Either let the long session drain to completion (bounded-drain budget sized for the longest legit utterance) OR KV-migrate it (playback-buffer-masked) to a fresh replica; long-form context needs pinned attention-sink + paged escape (L12), preserved across the move.
- If mishandled: Hard short-drain cuts the audiobook mid-chapter; or migrate without sink-pinning → the long-context model forgets on the new replica (StreamingLLM wraparound instability).

### SCALE-60 — Co-resident hybrid models share substrate without HoL
- Level: COMPOUND
- Pipeline: dots.tts (2-node nested) + CosyVoice2 (3-node) on one GPU
- Axes: co-residency:multi-model, capacity:duty, capacity:bottleneck
- Scenario: A nested AR+inner-CFM model and a 3-node DAG co-reside; both have CFM stages contending for compute.
- System must: Each model's stages carry their own duty entry; the shared compute ledger budgets both CFM loads (compute-bound, sublinear); admission rejects when EITHER model's bottleneck stage can't sustain another stream; the codec stages don't head-of-line-block their AR (decoupled per-stage batch).
- If mishandled: Admit on aggregate compute without per-bottleneck accounting → one model's CFM spike starves the other's CFM → both glitch.

### SCALE-61 — Spill respects per-tenant prefix-cache locality
- Level: COMPOUND
- Pipeline: cloned-voice TTS with radix prefix-cache
- Axes: spill:intra-node, multi-tenancy:isolation, cache:prefix
- Scenario: A cloned-voice tenant's streams reuse an 86%-hit ref-audio prefix (Fish-S2 L1); a spill would move a stream off the replica that holds its warm prefix.
- System must: Prefer spilling streams whose prefix is NOT yet warm (or cold zero-shot streams) so warm-prefix streams stay on their cache-hot replica; the destination's prefix-cache is per-tenant-salted (no cross-tenant reuse, G1).
- If mishandled: Spill the warm-prefix stream → it recomputes 86% cacheable ref-audio KV on the new replica → wasted work + worse TTFA exactly for the top commercial workload.

### SCALE-62 — Capacity planning: frame-budget admission across colocated stages
- Level: COMPOUND
- Pipeline: AR + nested-CFM + codec on one substrate
- Axes: capacity:frame-budget, capacity:duty, capacity:bandwidth
- Scenario: Plan the max concurrent streams for a model whose stages share one GB10 substrate + bus.
- System must: Compute N from the binding constraint across ALL colocated stages: `Σ compute_duty ≤ S` per substrate AND `Σ bandwidth_duty ≤ S·ceiling` on the shared pool (§6 conditions 2+3); the schedulable N is the min over stages, not the AR stage's N.
- If mishandled: Plan capacity off the AR stage alone → over-provision admission → the codec/bandwidth binds first → real capacity is lower than planned (under-delivery).

### SCALE-63 — Heterogeneous fleet: mixed edge + DC serving one tenant
- Level: COMPOUND
- Pipeline: STT
- Axes: fleet:mixed, scale:edge↔DC, worker:multi
- Scenario: A tenant's latency-critical calls should hit nearby GB10 edge; their bulk/batch should hit central B200.
- System must: Route by job class — single/latency → edge (one binary, Inline/Pipelined), batch/bulk → DC (Stage-batched, big N); the SAME model contract runs both; the LB knows each pool's slot/duty gauges; failover from edge → DC on edge saturation.
- If mishandled: One pool for both → latency calls queue behind batch on DC (HoL), or batch starves the small edge box (capacity mismatch).

### SCALE-64 — Mid-utterance never interrupted across a blue-green cutover
- Level: COMPOUND
- Pipeline: full-duplex S2S
- Axes: deploy:blue-green, lifecycle:long-session, control:no-mid-utterance
- Scenario: A full-duplex Moshi-class conversation is live on blue during a blue→green cutover.
- System must: Cut NEW conversations to green, keep the live full-duplex session on blue until the turn/conversation ends (short-drain, never mid-utterance), then retire blue; the full-duplex slot's user+model streams both drain together (F5 marker/flush, free slot only after the tail drains).
- If mishandled: Cut the live S2S session at the LB boundary → the conversation drops mid-turn (worse than truncation — a dropped duplex call).

### SCALE-65 — Burst + co-resident load + drain simultaneously
- Level: COMPOUND
- Pipeline: model A (bursting) + model B (loading) + replica draining
- Axes: scale:autoscale, co-residency:multi-model, deploy:drain
- Scenario: While a replica is draining for upgrade, model A bursts and an admin loads model B on a sibling.
- System must: The draining replica rejects new (correct), the burst spills to warm siblings + free slots, B's load on the sibling reserves footprint first (no OOM) and doesn't disturb that sibling's live A streams; the ledger tracks all three transitions concurrently.
- If mishandled: The drain + burst + load collide on capacity accounting → over-admit a sibling → A's live streams underrun during B's load.

### SCALE-66 — One-bad-request HoL prevention across tenants on shared encoder
- Level: COMPOUND
- Pipeline: token-AR STT, shared encoder stage
- Axes: multi-tenancy:fairness, fault:hol, capacity:prefill
- Scenario: One tenant submits a 147×-longer audio prompt to the shared STT encoder.
- System must: Force-chunk the long audio-prompt encode (`long_prefill_token_threshold`, #37308: 147× TTFT HoL) so the giant encode is sliced; other tenants' encodes interleave; the micro-batch collector keeps streaming/non-streaming out of the same batch (G-out-of-order).
- If mishandled: Run the 147× encode whole → it head-of-line-blocks the shared encoder → every tenant's STT TTFT spikes 147× (#37308).

### SCALE-67 — Degraded-mode capacity recalculation
- Level: COMPOUND
- Pipeline: AR + codec
- Axes: fsm:degraded, capacity:duty, admission:reject
- Scenario: An NPU fault degrades the codec stage to CPU (slower); the replica is `Degraded`.
- System must: Recalculate the codec stage's `T_step` (CPU is ~24× slower for AR but feedforward codec is OK, §1.7), LOWER the slot ceiling so the now-slower bottleneck still fits the frame budget, reject the excess, keep serving the reduced count; drain toward NPU repair.
- If mishandled: Keep the old (NPU) slot ceiling in Degraded → the CPU codec can't sustain it → every stream on the replica underruns (admit-beyond-degraded-capacity).

### SCALE-68 — Cross-cohort warm-repurpose with calibration recheck
- Level: COMPOUND
- Pipeline: model A (idle warm) → model B (hot)
- Axes: scale:autoscale, co-residency:repurpose, lifecycle:calibration
- Scenario: Repurpose A's idle warm replica to B; B has a persisted stamp for THIS device but a stale driver.
- System must: Unload A (zero slots), load B, re-validate B's calibration stamp against the current sha×device×driver (recalibrate if the driver key changed), warm B, flip Ready; faster than cold-spin but NOT skipping the stamp check.
- If mishandled: Reuse B's stale-driver stamp → B serves with wrong admission timings → frame misses on the repurposed replica.

### SCALE-69 — Multi-rate, multi-tenant cohort packing
- Level: COMPOUND
- Pipeline: 12.5 Hz TTS (tenant A) + 25 Hz STT (tenant B) + 75 Hz codec model (tenant C)
- Axes: capacity:cohort, multi-tenancy:isolation, scale:DC
- Scenario: Three tenants at three frame-rates share one B200.
- System must: Three separate (model, frame-rate) cohorts, each its own slot table + admission; they share the GPU temporally via the duty ledger; per-tenant salt/quota across cohorts; the 75 Hz model (6.7 ms budget, near sub-realtime) gets the tightest admission (low frame-rate is the biggest throughput lever, §4.4).
- If mishandled: Pack all three into one batch loop → no common tick → the 75 Hz stream paces the 12.5 Hz or vice-versa → cross-tenant cadence corruption.

### SCALE-70 — Rollout halts on a bad calibration stamp fleet-wide
- Level: COMPOUND
- Pipeline: any
- Axes: deploy:rolling, lifecycle:gate, lifecycle:calibration
- Scenario: During a rolling upgrade, the new model's accuracy/MOS gate fails on replica #3.
- System must: Fail-closed on #3 (refuse to flip Ready, keep #3 on the old model or drain it out), HALT the rollout (don't propagate the bad version to #4..#40), alert; the TTS gate includes a perceptual/MOS check (catches the WER-flat/MOS-crash signature, §5.2).
- If mishandled: Gate is offline-WER-only → the MOS-crashing model passes the gate → the rollout proceeds fleet-wide shipping degraded audio (C4).

### SCALE-71 — Spill chooses the cohort-matching replica
- Level: COMPOUND
- Pipeline: AR
- Axes: spill:intra-node, capacity:cohort
- Scenario: A stream must spill; sibling GPU-1 already runs the SAME (model, frame-rate) cohort with a free slot, sibling GPU-2 runs a different model.
- System must: Spill to GPU-1 (same cohort → admits into an existing warm slot table, no new cohort spin-up) over GPU-2 (would need to load/cold-start the model); the placer prefers warm-cohort locality.
- If mishandled: Spill to GPU-2 → cold-start the model just to absorb one stream → the spilled stream eats the load cliff.

### SCALE-72 — Bounded drop-oldest on a slow downstream consumer
- Level: COMPOUND
- Pipeline: AR → streaming WS egress
- Axes: capacity:backpressure, fault:slow-consumer
- Scenario: One tenant's network is slow; the server keeps generating audio faster than the client drains.
- System must: Bound the per-stream egress with drop-oldest (H2: HWM=0 → GBs of stale audio); back-pressure parks the upstream AR slot via bounded inter-stage queues (never silently buffer unbounded), disable WS write-coalescing (F10: per-frame flush); if the stream can't keep cadence, free its slot.
- If mishandled: HWM=0 unbounded → one slow consumer accumulates GBs of stale worthless audio → OOM takes the replica down for everyone.

### SCALE-73 — Admission across a substrate that hosts NPU + GPU stages
- Level: COMPOUND
- Pipeline: AR (GPU) + encoder (NPU) on GB10
- Axes: capacity:duty, placement:heterogeneous, admission:per-substrate
- Scenario: New streams load both the GPU (AR) and the NPU (encoder); the substrates don't share compute but share the bus.
- System must: Apply condition (2) PER substrate (NPU & GPU compute don't share) and condition (3) on the shared bandwidth pool (§6); a stream is admitted only if BOTH the GPU AR-duty AND the NPU encoder-duty AND the shared bandwidth all fit.
- If mishandled: One global compute-duty number → admit on GPU headroom while the NPU encoder is saturated → the encoder stage glitches (mixed-substrate admission error).

### SCALE-74 — Fan-in DAG doesn't deadlock under partial branches at scale
- Level: COMPOUND
- Pipeline: STT → (translate | passthrough) → TTS, many streams
- Axes: capacity:dag, fault:deadlock, scale:DC
- Scenario: Some streams are text-only (no audio-encoder output); the merge stage statically waits for [stt, translate, tts].
- System must: Use dynamic `wait_for_fn(req)` computing the expected source set PER stream (G11) so text-only streams don't deadlock the merge; routing stays within static topology (analyzable); multi-terminal for text vs text+audio.
- If mishandled: Fixed `wait_for=[a,b,c]` → text-only streams block the merge forever → the whole DAG wedges under mixed traffic (G11 fan-in deadlock).

### SCALE-75 — Replica failure mid-flight fails its streams, fleet absorbs
- Level: COMPOUND
- Pipeline: AR
- Axes: fault:replica-death, scale:autoscale, worker:multi
- Scenario: A replica hard-crashes (GPU fault) with 50 live streams.
- System must: The 50 streams fail-fast (their clients reconnect), the LB stops routing to the dead replica (health/sentinel, H6), the autoscaler replaces it (warm-repurpose or cold-spin), surviving replicas absorb reconnects into headroom; no fleet-wide cascade.
- If mishandled: LB keeps routing to the dead replica (liveness-only health) → reconnects also fail → cascading failure as load piles on a black hole.

### SCALE-76 — Quota + fairness + aging together under a greedy tenant
- Level: COMPOUND
- Pipeline: lockstep
- Axes: multi-tenancy:quota, multi-tenancy:fairness, admission:aging
- Scenario: A greedy tenant floods admissions; a quiet premium tenant has an occasional gold stream.
- System must: Per-tenant quota caps the greedy tenant's slots, FCFS-within-pool + aging promotes the waiting gold stream after `max_wait`, gold's SLO tier gets reserved duty; the greedy tenant gets its quota but no more, the gold stream is never starved.
- If mishandled: Quota without aging → the gold stream waits behind the greedy tenant's quota'd-but-continuous load; aging without quota → the greedy tenant still floods.

### SCALE-77 — Hot-swap a co-resident model while a sibling model bursts
- Level: COMPOUND
- Pipeline: model A v1→v2 hot-swap + model B burst, same box
- Axes: deploy:hot-swap, co-residency:multi-model, scale:autoscale
- Scenario: Box hosts A and B; A is being version-swapped while B suddenly bursts.
- System must: A-v2 loads with footprint reservation (accounting B's current live reservation too), A's v1 streams drain, B's burst is admitted only up to the shared substrate's remaining duty (the swap's transient extra VRAM/duty is reserved, so B's burst can't over-admit into space A-v2 needs).
- If mishandled: B's burst admitted into VRAM that A-v2's load needs → A-v2 load OOMs → crash takes both A and B down.

### SCALE-78 — Tail-latency control: capture largest cohort first
- Level: COMPOUND
- Pipeline: AR, multiple cohort sizes
- Axes: lifecycle:warmup, kernel:cuda-graph, capacity:cohort
- Scenario: A replica must serve cohorts at B∈{1,2,4,…,64}; CUDA-graphs are captured per cohort size.
- System must: Capture the LARGEST cohort graph FIRST (H4) so the shared graph pool's peak is reserved up-front (no later capture OOM under load), capture EXACT slot counts (1,2,4,…) = zero padding (sidesteps the 257→512 power-of-2 cliff), serve first request eager (lazy/background capture).
- If mishandled: Capture-on-demand largest-last → capture OOMs after smaller graphs already allocated → crash-loop under growing load (#44209 sm120).

### SCALE-79 — Drain budget sized to the longest legitimate utterance
- Level: COMPOUND
- Pipeline: mixed short calls + long-form TTS
- Axes: deploy:drain, lifecycle:long-session, control:no-mid-utterance
- Scenario: A drain must finish in bounded time but a 15-min audiobook is mid-generation.
- System must: Size the short-drain budget to the longest LEGITIMATE utterance class (long-form gets a longer budget than IVR), then abort; never unbounded (a wedged stream can't hold the drain forever, H7) and never mid-utterance for normal calls.
- If mishandled: Flat short drain → audiobooks truncated; flat long drain → one wedged stream stalls the deploy indefinitely.

### SCALE-80 — Per-tenant duty accounting prevents cross-tenant priority inversion
- Level: COMPOUND
- Pipeline: tenant-A realtime + tenant-B realtime, shared cohort
- Axes: multi-tenancy:fairness, capacity:duty, fault:priority-inversion
- Scenario: Tenant A and B share a cohort; A's streams have longer context (heavier per-step), consuming more duty per slot.
- System must: Account duty PER stream/tenant (heavier-context streams cost more duty, L10: token-count ≠ compute), so admission charges A's heavy streams their true cost; B isn't squeezed by an under-counted A.
- If mishandled: Flat per-slot duty (ignoring context cost) → A's heavy streams under-charged → the cohort over-admits → B's streams underrun (the priority inversion is hidden in the duty model).

---

## EXTREME

### SCALE-81 — 5000-call flash crowd, 6 variants, per-tenant SLO, rolling upgrade
- Level: EXTREME
- Pipeline: 6 model variants (STT/TTS/S2S) across a B200 fleet
- Axes: scale:autoscale, burst:flash-crowd, deploy:rolling, multi-tenancy:slo, co-residency:multi-model, worker:multi
- Scenario: 5000 concurrent calls hit an autoscaling B200 fleet co-hosting 6 model variants with per-tenant SLOs DURING a rolling model upgrade.
- System must: (a) Warm headroom + intra-node spill absorb the leading edge; (b) reject-don't-glitch the true overflow with Retry-After + slot/duty gauges driving aggressive warm-repurpose-then-cold-spin; (c) the rolling upgrade PAUSES (don't drain more replicas during a flash crowd — the shrunken pool can't take it); (d) gold tenants admitted preferentially, silver shed first, no tier starved (aging); (e) each variant's bottleneck-stage + shared-bandwidth ledgers stay budgeted; (f) existing streams keep cadence (no mid-utterance preempt, playback-buffer-masked any migration). The optimal KISS move is to FREEZE the rollout, scale on warm capacity, and shed by SLO tier.
- If mishandled: Drain-during-burst collapses the pool → mass rejection of even gold; or admit-and-degrade 5000 → fleet-wide underrun; or load-without-reservation OOMs cascade as variants thrash.

### SCALE-82 — Correlated reconnect storm after a fleet-wide blip
- Level: EXTREME
- Pipeline: any
- Axes: scale:autoscale, fault:reconnect-storm, burst:flash-crowd, worker:multi
- Scenario: A transient network blip drops 10k sessions fleet-wide; all clients reconnect within 2 s (thundering herd).
- System must: Treat the reconnect storm as a flash crowd (warm headroom + spill + reject-with-jittered-Retry-After to de-correlate), NEVER scale-to-zero so warm capacity exists to catch it; the storm-control/governor caps reconnect admission rate per replica so the herd doesn't self-amplify into a second outage.
- If mishandled: No jitter/cap → all 10k retry simultaneously → synchronized rejection → synchronized retry → standing-wave overload (metastable failure).

### SCALE-83 — Cascading OOM from load-while-serving across co-resident variants
- Level: EXTREME
- Pipeline: 6 variants, traffic-driven hot-set rotation
- Axes: co-residency:multi-model, lifecycle:oom-guard, fault:cascade
- Scenario: Traffic rotates the hot set faster than TTL evicts; the loader tries to bring up variant #5 and #6 while #1..#4 are at high occupancy and the autoscaler is mid-spin.
- System must: Serialize loads behind pre-admission footprint reservation (H3); refuse #5/#6 loads that don't fit ALONGSIDE live reservations (typed error, route those tenants to a sibling/new replica); never start-then-OOM; the duty+memory ledger is the single arbiter so two concurrent loads can't both "fit" the same VRAM.
- If mishandled: Two loads race the same free VRAM → both commit → OOM mid-capture → crash → the crash frees VRAM → autoscaler reschedules the same race on the replacement → crash-loop cascade.

### SCALE-84 — Rolling upgrade + driver upgrade + heterogeneous fleet at once
- Level: EXTREME
- Pipeline: AR-TTS across GB10 edge + H200 + B200
- Axes: deploy:rolling, fleet:mixed-gpu, fleet:mixed-driver, lifecycle:calibration
- Scenario: A model upgrade rolls out while half the fleet also gets a CUDA-driver bump, across three GPU generations.
- System must: Each replica recalibrates on its OWN sha×device×driver×warm-set key (no cross-key reuse), gate-fail-closed per replica + HALT the rollout on the first MOS/duty gate failure of any generation, keep the LB routing only to Ready+warm+calibrated replicas; the heterogeneous admission math stays per-replica (GB10 latency-only vs B200 big-batch).
- If mishandled: Share calibration across generations/drivers → admission math wrong on the diverse replicas → frame misses scattered unpredictably across the fleet (un-diagnosable rollout).

### SCALE-85 — Full-duplex S2S fleet upgrade with zero dropped conversations
- Level: EXTREME
- Pipeline: Moshi-class full-duplex S2S
- Axes: deploy:rolling, lifecycle:long-session, control:no-mid-utterance, fsm:draining
- Scenario: A full-duplex S2S model fleet (multi-minute conversations, user+model streams modeled per F6) must upgrade with no conversation dropped.
- System must: Per replica: stop new conversations, let live duplex sessions run to conversation-end (the longest residency class — drain budget sized accordingly), KV-migrate only if necessary (playback-buffer-masked, both streams' state moved atomically), then upgrade; warm headroom on green absorbs new conversations; barge-in/turn-taking stays correct throughout (reliable abort, G9).
- If mishandled: Drain-cut a multi-minute duplex call → a dropped live conversation (the worst UX failure); or migrate one direction's KV but not the other → desync'd full-duplex.

### SCALE-86 — Bandwidth-saturated GB10 edge cluster under co-resident pressure
- Level: EXTREME
- Pipeline: AR-TTS + STT + codec, all on GB10 unified memory
- Axes: scale:edge, capacity:bandwidth, co-residency:multi-model, substrate:unified
- Scenario: An edge GB10 cluster serves 3 co-resident models; aggregate demand pushes the shared 273 GB/s LPDDR bus to its ceiling on every box.
- System must: The shared-bandwidth ledger (§6 cond 3) is the BINDING constraint — admit only while `Σ bandwidth_duty ≤ S·ceiling`; place memory-bound AR to overlap with compute-bound codec where possible, co-locate+time-share when all saturate; reject overflow (the bus can't be autoscaled per-box) and shed to a sibling/DC; quantize weights (universal bandwidth win, §2.1).
- If mishandled: Compute-only admission ignores the bus → every box oversubscribes its one ceiling → all three models stall fleet-wide (the unified-memory contention trap §3.4).

### SCALE-87 — Variable-stride / variable-NFE models break naive cohorting at scale
- Level: EXTREME
- Pipeline: FlashTTS-class (MTP-3 + 2-NFE) + DiTAR-class (patch-AR + variable inner-NFE)
- Axes: capacity:cohort, capacity:duty, scale:DC
- Scenario: New models advance by a variable stride and run a per-stream RUNTIME-DIAL inner NFE (L5); thousands of streams pick different NFE.
- System must: Cohort by (model, frame-rate) but tolerate variable stride; the nested inner solve composes a per-stream variable-NFE micro-batch INSIDE one AR step (compose two batchers per step, not pick one); the duty ledger charges each stream its own inner-NFE cost; admission uses the per-stream predicted (not flat) step time.
- If mishandled: Assume fixed-stride/fixed-NFE lockstep → streams at different NFE can't share a tick → either they desync or the engine forces a uniform NFE (quality regression for the dialed-up streams).

### SCALE-88 — Dynamic-frame-rate codec fleet defeats fixed cohort keys
- Level: EXTREME
- Pipeline: FlexiCodec-class (3–12.5 Hz, data-dependent per-frame, L6)
- Axes: capacity:cohort, scale:DC, capacity:duty
- Scenario: A codec's frame-rate varies per-utterance AND per-frame, not known a-priori; thousands of streams.
- System must: The cohort key tolerates unknown-a-priori rates (don't pre-bin by a static FR); lockstep "advances a model-dependent variable stride" per tick; admission budgets the WORST-case frame-rate's duty (tightest budget) so a stream that speeds up to 12.5 Hz mid-utterance still fits; re-cohort/re-admit on sustained rate change.
- If mishandled: Static (model, frame_rate) cohort assumption → a stream that shifts rate mid-utterance is in the wrong cohort → its tick desyncs the batch (L6: §4.2/§5.1 insufficient for dynamic-rate).

### SCALE-89 — Metastable overload: graceful degradation beats reject-storm
- Level: EXTREME
- Pipeline: AR + codec
- Axes: overload:metastable, admission:graceful, scale:DC
- Scenario: Load hovers at 110% of capacity for minutes; pure reject-everything causes retry storms that keep the system pinned.
- System must: Deadline-aware GRACEFUL degradation as primary (Niyama: 95%+ deadlines @ 50% overload vs <20% reject; BrownoutServe quality-brownout, L9) — relegate to a degraded queue / lower-quality tier, protect cadence via the client playback buffer, hard-reject only at TRUE saturation; warm over-provisioning so the degraded band exists; jittered Retry-After to break retry synchronization.
- If mishandled: Crude reject-don't-glitch as the ONLY tool → reject → retry → reject standing wave → metastable collapse (L9: reject is the crude baseline deadline-aware schedulers beat).

### SCALE-90 — Prefix-cache poisoning attempt across tenants under load
- Level: EXTREME
- Pipeline: cloned-voice TTS, shared radix prefix-cache, multi-tenant
- Axes: multi-tenancy:isolation, security:salt, cache:prefix, scale:DC
- Scenario: A malicious tenant crafts requests with identical token-ids but different injected ref-audio to try to read/poison another tenant's cached voice KV under high concurrency.
- System must: Prefix-cache key = sha256 (NEVER xxhash, collision = cross-tenant leak) + per-tenant `cache_salt` on block-0 + content-hash of ALL injected codebooks (G1 blake2b over every channel, cb0-only collides); zero-shot → `extra_key=None` so legit sharing survives; the attacker's salted+content-hashed key never collides with the victim's.
- If mishandled: Token-id-only or cb0-only or xxhash key → attacker reads/contaminates the victim's voice KV → wrong-voice/leaked-audio output under concurrency (G1/L1 privacy disaster).

### SCALE-91 — Cohort-wide NaN/illegal-memory blast contained at 4000 slots
- Level: EXTREME
- Pipeline: lockstep, 4000 slots across a fleet
- Axes: fault:nan, multi-tenancy:fault-isolation, scale:DC
- Scenario: Across a fleet running 4000 lockstep slots, a handful of slots hit NaN logits / would read a sentinel-poisoned KV cell in the same frame.
- System must: Per-frame always-on NaN reduction (H1) + masked-row input substitution (F1: force BOS/`initial` so the dense kernel never reads sentinel `-2`/stale) on EVERY batch → reject only the offending frames, the other 3990+ slots emit correctly; one poisoned row can't take its 64-wide batch (let alone the fleet).
- If mishandled: One unsubstituted masked row → CUDA illegal-memory kills its whole 64-slot batch (F1) → replicated across the fleet → mass simultaneous batch deaths from a few bad inputs.

### SCALE-92 — Slot-leak erosion under sustained churn at fleet scale
- Level: EXTREME
- Pipeline: lockstep, high session turnover
- Axes: lifecycle:slot-free, fault:leak, scale:DC, worker:multi
- Scenario: Millions of short sessions/day churn through the fleet; any per-session leak compounds over days.
- System must: Multi-trigger slot-free from INSIDE the step loop on ANY of {receiver closed, sender disconnected, send error, ping-timeout 20 s, idle-timeout 120 s} (F9) + transactional reset_slot + cap EVERY per-session bookkeeping map (G6: 10k→trim-5k) + purge per-slot maps on free; an open-slots gauge + per-conn step counter detect any residual leak.
- If mishandled: Rely on a single disconnect callback (can be missed, F9) or an unbounded `_closed`/`_aborted` set (G6) → slots + memory leak slowly → effective capacity decays over days → silent capacity loss culminating in a 3-day-uptime crash.

### SCALE-93 — Spill rebalance across a draining + bursting + degraded fleet
- Level: EXTREME
- Pipeline: AR
- Axes: spill:kv-migration, deploy:drain, fsm:degraded, scale:autoscale
- Scenario: Simultaneously: 5 replicas draining for upgrade, a burst on the live ones, and 2 replicas in Degraded (NPU fault, lower ceiling).
- System must: Route new to Ready+full-capacity replicas first, spill from draining/degraded toward them (intra-node first, KV-migrate playback-buffer-masked), respect each Degraded replica's LOWERED ceiling in admission, freeze/slow the drain if the burst threatens the pool, autoscale to restore headroom; no stream interrupted mid-utterance through all of it.
- If mishandled: Spill INTO a draining or degraded replica (stale ceiling) → it can't sustain → the migrated stream glitches on arrival; or drain-through-burst collapses the pool.

### SCALE-94 — Calibration thrash from rapid sha/driver churn during canary fan-out
- Level: EXTREME
- Pipeline: any, rapid canary iterations
- Axes: lifecycle:calibration, deploy:canary, scale:DC
- Scenario: A team rapidly iterates canary builds (new sha every few minutes) across many replicas, each needing fresh calibration.
- System must: Persist calibration stamps keyed by sha×device×driver×warm-set so a re-deployed sha hits the cache (no re-calibrate); only genuinely-new shas calibrate; calibration runs WITHOUT a profiler and excludes first-request (catalog §B) so the stamps are trustworthy; the canary's calibration never blocks stable's serving (separate cohort).
- If mishandled: Re-calibrate every sha from scratch with no cache → calibration storms saturate the fleet's spare compute → the canary fan-out itself degrades stable's SLO (calibration as a noisy neighbor).

### SCALE-95 — Mixed edge+DC failover when an entire edge region drops
- Level: EXTREME
- Pipeline: STT + TTS
- Axes: fleet:mixed, scale:edge↔DC, fault:region-down, worker:multi
- Scenario: An entire edge region (many GB10 boxes) goes offline; all its latency-critical streams must fail over to central B200 DC.
- System must: Fail the region's streams over to DC (same model contract, one binary — DC runs Stage-batched), DC absorbs them into warm headroom + autoscale (B200 big-batch is well-suited to the sudden concurrency), reject-with-Retry-After only the true overflow; latency degrades (edge→DC RTT) but calls survive; never scale-to-zero meant DC warm capacity exists to catch the failover.
- If mishandled: No DC warm headroom (DC sized only for its own batch load) → the edge failover is mass rejection → a regional edge outage becomes a total outage for those users.

### SCALE-96 — Per-tenant SLO honored while a tenant DoSes admission
- Level: EXTREME
- Pipeline: lockstep, multi-tenant
- Axes: multi-tenancy:slo, multi-tenancy:quota, fault:dos, scale:DC
- Scenario: One tenant floods admission requests (effectively a DoS) while gold tenants need their tight-TTFA SLO honored across the fleet.
- System must: Per-tenant admission rate-limit + slot quota cap the flooder, reserved duty/slots per SLO tier protect gold, aging prevents any non-flooder starving, the flooder's excess is rejected at the EDGE of admission (cheap, before touching the scheduler) so the flood doesn't even cost the hot path; gold's bottleneck-stage budget stays reserved.
- If mishandled: No per-tenant rate-limit → the flood consumes admission CPU + crowds the slot pool → gold's SLO breaches because one tenant weaponized the shared admission path.

### SCALE-97 — Six-variant B200 box: keep-alive + version-swap + burst + degrade together
- Level: EXTREME
- Pipeline: 6 variants on one B200
- Axes: co-residency:multi-model, deploy:hot-swap, lifecycle:ttl, fsm:degraded, scale:autoscale
- Scenario: On one B200: variant-1 is being version-swapped, variant-2's TTL is expiring, variant-3 bursts, and an MPS partition degrades — all within a minute.
- System must: The single memory+duty ledger arbitrates ALL of it — v1-v2 load reserves footprint, v2's TTL-evict only with zero slots, v3's burst admitted only to remaining duty, the degraded partition lowers its variants' ceilings; no operation starts that the ledger can't fully reserve; live streams across all 6 variants keep cadence.
- If mishandled: Independent uncoordinated handlers each "see" free VRAM/duty → concurrent commits over-subscribe → OOM or frame-budget collapse takes multiple variants down at once.

### SCALE-98 — Standing-wave from synchronized autoscale + cold-start
- Level: EXTREME
- Pipeline: any
- Axes: scale:autoscale, lifecycle:cold-start, fault:standing-wave
- Scenario: An aggressive autoscaler spins many cold replicas simultaneously on a spike; their synchronized 1.7–12.8 s cold-starts all complete at once, then traffic dips, triggering synchronized scale-down, then another spike.
- System must: Warm over-provisioning + warm-repurpose make cold-spin the LAST resort (so cold-start rarely gates a spike, L9); scale-down hysteresis + minimum-warm-floor (never-to-zero) damp the oscillation; stagger cold-starts (don't synchronize the herd); readiness-gate each new replica (no traffic until warm+calibrated).
- If mishandled: Synchronized cold-spin → all become Ready together → over-capacity → synchronized scale-down → next spike repeats → autoscaler oscillation (standing wave) with cold-start cliffs on every upswing.

### SCALE-99 — Fleet-wide rollout poisoned by a silent MOS regression
- Level: EXTREME
- Pipeline: AR-TTS, full fleet
- Axes: deploy:rolling, lifecycle:gate, fault:silent-regression
- Scenario: A new TTS checkpoint passes offline WER but has a subtle MOS/AR-drift regression (WER-flat/MOS-crash signature, §5.2) that an offline-only gate misses.
- System must: The readiness gate MUST include the perceptual/MOS check + streaming-playback + concurrent-load layers (C4 validation pyramid), fail-closed on the first replica, HALT the rollout fleet-wide, auto-rollback to the last verified stamp; the per-replica accuracy stamp gates serving, not just CI.
- If mishandled: Offline-WER-only gate (layer-1 only) → the MOS-crashing model passes → rolls out fleet-wide → every user hears degraded audio while every metric is green (the exact bug §5.2/C4 warns of).

### SCALE-100 — Total saturation: graceful shed hierarchy holds the SLO floor
- Level: EXTREME
- Pipeline: AR + codec, multi-tenant, multi-SLO
- Axes: overload:total, admission:shed, multi-tenancy:slo, scale:DC
- Scenario: Demand far exceeds the fully-autoscaled fleet's ceiling (e.g. a viral event); even max warm + cold capacity is exhausted.
- System must: Execute the shed hierarchy in order — stop admitting → shed Batch → shed silver Realtime ≤1/tick (60 s hysteresis) → protect gold's reserved floor (FR-S3b); deadline-aware relegation keeps the most at-risk-but-savable streams (VoxServe risk-scheduling), reject the rest with honest Retry-After; NEVER admit-and-degrade everyone (P-4); the gold floor and existing-stream cadence survive the event.
- If mishandled: Admit-and-degrade to "serve" everyone → universal underrun (all tiers fail at once); or shed gold equally with silver → premium SLO violated during the one event that matters most.

### SCALE-101 — Cross-region active-active with KV-migration and split-brain guard
- Level: EXTREME
- Pipeline: AR-TTS, two regions
- Axes: scale:DC, spill:kv-migration, fault:split-brain, worker:multi
- Scenario: Two active-active regions load-balance one tenant; a network partition makes each region think the other is down.
- System must: Per-region slot/duty gauges drive independent admission (each region rejects-don't-glitch on its own saturation); a session is OWNED by one region at a time (monotonic ownership token / channel_id, F3) so a partition can't double-admit the same session into both; KV-migration only on a clean handoff (playback-buffer-masked), never speculatively across a partition.
- If mishandled: Both regions admit the same reconnecting session (split-brain) → duplicate slots + double-billing + contradictory output; or migrate across the partition → lost/duplicated KV.

### SCALE-102 — Warmup-cliff thundering herd after a fleet-wide restart
- Level: EXTREME
- Pipeline: any
- Axes: lifecycle:warmup, lifecycle:cold-start, fault:thundering-herd, scale:autoscale
- Scenario: A bad config push forces a fleet-wide rolling restart; every replica must reload+warm+calibrate while traffic keeps arriving.
- System must: `/readyz` 503 until warm+calibrated on EVERY replica (no traffic to unwarmed, C7) → the LB holds/queues briefly + clients see honest 503+Retry-After; restart STAGGERED (never all-at-once) so warm capacity always exists; warm-repurpose + minimum-warm-floor mean some replicas stay up to absorb the herd; readiness-gated re-entry.
- If mishandled: Restart all at once with /readyz 200 on process-up → traffic floods unwarmed replicas → every caller hits the first-request capture cliff simultaneously → fleet-wide dead-air (the warmup-cliff at fleet scale).

### SCALE-103 — Per-stage crash-storm isolation under a poison-input flood
- Level: EXTREME
- Pipeline: STT encoder (separate process) + AR (separate process), fleet-wide
- Axes: fault:crash-storm, deploy:process-topology, multi-tenancy:fault-isolation, scale:DC
- Scenario: A flood of malformed audio crashes the encoder process on many replicas near-simultaneously.
- System must: Encoder-as-separate-process (G3/G7) contains each crash to the encoder's in-flight requests (AR cohorts survive); 3-layer crash detection (scheduler handler + done-callbacks + liveness, G7) fails those requests fast; auto-restart the encoder process (bounded restart with backoff to avoid a crash-loop), drop the poison inputs at ingress validation so the restart doesn't immediately re-crash.
- If mishandled: Colocated encoder+AR → each crash takes the whole process group → a poison flood becomes a fleet-wide AR outage; or unbounded restart → crash-loop storm.

### SCALE-104 — Heterogeneous-fleet capacity planning under a demand shift
- Level: EXTREME
- Pipeline: AR-TTS + STT, mixed GB10/H200/B200
- Axes: capacity:frame-budget, fleet:mixed-gpu, scale:autoscale, capacity:duty
- Scenario: Demand shifts from many-small-edge-calls toward few-large-batch (or vice-versa) across a fleet with three GPU classes and different batch-knees.
- System must: Plan/autoscale per substrate's batch-knee (1–4 CPU/edge, 64–512 DC, §2.2) — route latency-small to GB10 (knee small, latency-only), batch-large to B200 (knee large, fill the SMs), recompute each class's admissible N from its OWN calibrated frame-budget+duty; rebalance routing as demand shifts rather than forcing one batch profile everywhere.
- If mishandled: One global batch/placement policy → small calls on B200 (severe under-occupancy, wasted silicon) or large batch on GB10 (blow the 273 GB/s ceiling) → mis-provisioned on both ends of the demand shift.

### SCALE-105 — Drain-then-abort race with in-flight migration at scale
- Level: EXTREME
- Pipeline: AR
- Axes: deploy:drain, spill:kv-migration, fault:race, scale:DC
- Scenario: Many replicas drain simultaneously; some streams are mid-KV-migration to other (also-draining or bursting) replicas when the drain deadline hits.
- System must: The drain budget covers the migration completion (don't abort a stream MID-migration → that loses its KV entirely); ownership is single-writer (the source holds the stream until the destination ACKs the migrated KV, then frees, G4 notify-before-wait); if the deadline truly expires, abort cleanly (client reconnects) rather than leaving a half-migrated zombie slot.
- If mishandled: Abort mid-migration → KV lost on both ends → the stream dies AND leaks a slot on the destination (half-migrated zombie) → fleet-wide slot erosion during the upgrade.

### SCALE-106 — Bandwidth-fair multi-tenancy on a saturated unified-memory edge cluster
- Level: EXTREME
- Pipeline: 3 tenants, 3 models, GB10 unified memory
- Axes: multi-tenancy:fairness, capacity:bandwidth, substrate:unified, scale:edge
- Scenario: Three tenants' co-resident models all push the shared GB10 LPDDR bus to saturation; one tenant's memory-bound AR streams crowd the bus.
- System must: The shared-bandwidth ledger fairly partitions `S·ceiling` ACROSS tenants (not just total), charging each tenant's stages their bandwidth_duty; a bandwidth-hungry tenant is capped at its fair bus share so others' frame budgets hold; prefer overlapping a memory-bound tenant's AR with a compute-bound tenant's codec (§3.4).
- If mishandled: Bandwidth ledger is total-only (not per-tenant) → one tenant monopolizes the bus within its compute quota → other tenants' streams underrun (noisy-neighbor at the BANDWIDTH layer, the unified-memory-specific trap).

### SCALE-107 — Coordinated 6-variant rolling upgrade preserving co-residency invariants
- Level: EXTREME
- Pipeline: 6 variants, fleet-wide
- Axes: deploy:rolling, co-residency:multi-model, lifecycle:fsm, scale:DC
- Scenario: All 6 co-resident variants get a coordinated upgrade; on each box the 6 must swap without ever exceeding VRAM (can't hold old+new of all 6 at once).
- System must: Per-box, upgrade variants ONE at a time (load v2 of variant-i reserving footprint, drain v1-i, unload v1-i, next) so peak VRAM = (5 old + 1 new) not (6 old + 6 new); the box stays Ready throughout for the 5 not-currently-swapping variants; the ledger forbids starting variant-i+1's swap until variant-i's old is freed.
- If mishandled: Swap all 6 at once → need 12 model footprints → OOM → the whole box crashes mid-upgrade taking all 6 variants' live streams down.

### SCALE-108 — Flash crowd of long-form sessions exhausts drain+migration headroom
- Level: EXTREME
- Pipeline: long-form TTS (audiobooks)
- Axes: burst:flash-crowd, lifecycle:long-session, deploy:drain, spill:kv-migration
- Scenario: A flash crowd of LONG-form sessions (15-min each) lands during a rolling upgrade — long residency means they don't drain quickly to free replicas.
- System must: Recognize long-residency cohorts pin capacity (slots free slowly) → the autoscaler must add capacity for the BURST rather than wait for drains, PAUSE the rolling upgrade (draining a long-form replica takes 15 min — can't afford it during a burst), KV-migrate (sink-pinned + playback-masked, L12) only when truly necessary; reject overflow with Retry-After.
- If mishandled: Treat long-form like short calls (expect quick drain) → drain a long-form replica during the burst → either truncate audiobooks or block the upgrade for 15 min while the pool is under-capacity → cascading rejection.

### SCALE-109 — Risk-of-violation scheduling across 4000 streams at the deadline edge
- Level: EXTREME
- Pipeline: AR + codec, 4000 streams
- Axes: overload:graceful, admission:risk-scheduling, scale:DC
- Scenario: 4000 streams across the fleet are all near their TTFA/cadence deadline; the bottleneck stage is momentarily over-budget.
- System must: Schedule by RISK-OF-VIOLATION (VoxServe/Niyama soft-deadline, L3/L9) — service the most-at-risk-but-still-savable streams first, let already-safe streams coast (delivering early is worthless past the deadline), protect cadence via client playback buffers; shed/relegate the unsavable rather than glitch everyone; binary streaming-viability objective (deliver-in-time or it's worthless).
- If mishandled: FCFS/round-robin at the deadline edge → service order ignores risk → savable streams miss while already-safe streams get needless early frames → more total violations than necessary (L9: crude scheduling loses goodput).

### SCALE-110 — Whole-datacenter brownout: degrade quality fleet-wide, hold cadence
- Level: EXTREME
- Pipeline: AR-TTS
- Axes: overload:brownout, precision:tiered, scale:DC, capacity:duty
- Scenario: A DC-wide capacity shortfall (cooling/power cap throttles GPUs) cuts effective throughput fleet-wide with no time to add capacity.
- System must: Quality-brownout (BrownoutServe, L9) — drop to a cheaper precision tier / fewer NFE / smaller cohort to fit the throttled budget (74%→7% violations @ ~5% accuracy loss), recalibrate the reduced budget, hold cadence (no frame drops) at lower quality; recover quality as the throttle lifts; admission tracks the reduced ceiling.
- If mishandled: No brownout lever → the throttle forces frame-budget misses → universal underrun; or keep full quality + reject most → mass outage during the shortfall (brownout trades a little quality for staying up).

### SCALE-111 — Sustained metastable churn: hysteresis + headroom break the loop
- Level: EXTREME
- Pipeline: any
- Axes: overload:metastable, scale:autoscale, admission:shed, fault:oscillation
- Scenario: Demand oscillates around the capacity line for an hour; naive scale-up/down + shed/admit cycles amplify into a self-sustaining oscillation (load → shed → retry → load).
- System must: Hysteresis on BOTH admission-shed (60 s, FR-S3b) AND autoscale (don't scale down on a transient dip), warm-floor (never-to-zero) so the trough doesn't shed warm capacity needed for the next peak, jittered Retry-After to de-correlate retries, deadline-aware relegation (not hard reject) to avoid retry storms; the goal is to DAMP, not chase, the oscillation.
- If mishandled: No hysteresis → scale/shed chases every wiggle → control-loop resonance → the system spends the hour oscillating between over- and under-capacity instead of settling (metastable failure mode).

### SCALE-112 — Multi-tenant, multi-SLO, multi-variant steady-state at B200 fleet scale
- Level: EXTREME
- Pipeline: 6 variants, hundreds of tenants
- Axes: multi-tenancy:slo, multi-tenancy:quota, co-residency:multi-model, capacity:duty, scale:DC
- Scenario: Steady-state production: hundreds of tenants, gold/silver SLOs, 6 co-resident variants, thousands of streams, normal churn — the everyday EXTREME the design must hold indefinitely.
- System must: Per-(model,frame-rate) cohorts + per-tenant salt/quota + per-SLO-tier reserved duty + per-substrate compute ledger + shared-bandwidth ledger + keep-alive TTL/LRU + aging + bounded bookkeeping + multi-trigger slot-free, all composing without any single-point over-subscription; the ledger is the one arbiter; the fleet holds SLO for days without leak, thrash, or starvation.
- If mishandled: Any one mechanism missing (no aging → starvation; no bandwidth ledger → unified-mem contention; no bookkeeping cap → slow leak; no per-tenant salt → cross-tenant leak) → the steady-state silently degrades until a tail event exposes it.

### SCALE-113 — Rollback under in-progress flash crowd (abort the upgrade safely)
- Level: EXTREME
- Pipeline: AR-TTS, fleet-wide
- Axes: deploy:rolling, deploy:rollback, burst:flash-crowd, fsm:draining
- Scenario: A flash crowd hits mid-rolling-upgrade and the new version shows a live regression — the upgrade must ABORT and roll back while under burst load.
- System must: HALT forward rollout immediately, roll the already-upgraded replicas back to the last verified stamp (load v1 reserving footprint, drain v2, never mid-utterance), keep ALL replicas serving the burst throughout (rollback is itself a drain-and-swap that preserves cadence), warm-repurpose to hold capacity during the double churn; the verified-stamp cache makes rollback fast (no re-calibrate).
- If mishandled: Abort the upgrade by hard-cutting v2 replicas → drops live calls during the burst (worst time); or roll back without footprint reservation → OOM during the double load/burst → cascade.

### SCALE-114 — Frame-rate-spread admission on a box mixing 12.5 Hz and 150 Hz models
- Level: EXTREME
- Pipeline: Mimi-12.5 Hz TTS + EnCodec-48k-150 Hz model
- Axes: capacity:cohort, capacity:frame-budget, scale:DC
- Scenario: One box co-hosts a generous 12.5 Hz (80 ms budget) model and a punishing 150 Hz (6.7 ms budget, sub-realtime even at batch-1) model (§4.4 12× spread).
- System must: Separate cohorts; the 150 Hz cohort gets a TINY admissible N (its 6.7 ms budget barely fits one step — low frame-rate is the biggest throughput lever, §4.4) while the 12.5 Hz cohort batches 16–32; the duty ledger budgets both on the shared substrate; admit the 150 Hz model only where its frame budget is truly met (maybe edge/dedicated, not packed).
- If mishandled: Apply the 12.5 Hz cohort's generous N to the 150 Hz model → it can't meet 6.7 ms → every 150 Hz stream underruns (frame-rate-blind admission).

### SCALE-115 — Graceful whole-fleet drain for a datacenter maintenance window
- Level: EXTREME
- Pipeline: all variants, all sessions
- Axes: deploy:drain, scale:edge↔DC, fault:region-down, control:no-mid-utterance
- Scenario: A planned full-DC maintenance window requires draining every replica while preserving as many live sessions as possible by failing them over.
- System must: Region-by-region (never all-at-once): mark replicas Draining (503 to new), fail live sessions over to the OTHER region/edge via KV-migration (playback-masked) where capacity allows OR drain them to completion (long-form gets its budget), short-drain-then-abort the remainder cleanly (clients reconnect to the surviving region); the surviving region's warm headroom + autoscale absorbs the migrated load.
- If mishandled: Drain the whole DC at once → no failover target → mass mid-utterance drops; or migrate everything blindly → overwhelm the surviving region → it rejects the migrations → double failure.

### SCALE-116 — Calibration + warmup + capture all gated before a high-stakes cutover
- Level: EXTREME
- Pipeline: AR + nested-CFM
- Axes: lifecycle:fsm, lifecycle:calibration, lifecycle:warmup, deploy:blue-green
- Scenario: A high-stakes blue-green cutover for a model with nested CFM (inner-NFE folded into step time) must not expose ANY first-request cliff.
- System must: Green replicas complete Loading→Warming (2–3 full-mask steps + CUDA-graph capture largest-cohort-first OFF the hot path) → calibration (T_step including the nested inner-NFE, §3.3, under synthetic co-load, no profiler) → only then Ready; `/readyz` gates the cutover on ALL of {warm, captured, calibrated, accuracy-stamp-verified}; the LB shifts to green only when every green replica is fully Ready.
- If mishandled: Cut over on warm-but-not-calibrated (or captured) → first requests hit lazy capture (#44209) or wrong admission timings → the high-stakes cutover stutters at the worst moment (C7 first-request cliff).

### SCALE-117 — Idle-slot energy budget under heterogeneous residency at scale
- Level: EXTREME
- Pipeline: lockstep, mixed short + long sessions
- Axes: capacity:duty, fault:idle-waste, scale:DC, multi-tenancy:fairness
- Scenario: Under heterogeneous residency (barge-in pauses, async turn-taking), many slots in each batch are masked-idle; idle-lane energy ≈48% of serving energy (L8); padding 13%@BS1→40%@BS32.
- System must: Either compact/repack active slots into denser batches (L8: ragged/packed eliminates padding) OR explicitly budget the masked-slot energy/bandwidth cost in the duty ledger (masked slots are NOT free under heterogeneous residency, L8); admission/placement accounts the real (not idealized) batch density.
- If mishandled: Assume masked slots are free (the naive lockstep claim) → the duty ledger under-counts → admit beyond real density → the masked-but-still-computed rows blow the energy/bandwidth budget → frame misses + wasted ~48% energy fleet-wide.

### SCALE-118 — Hybrid-KV capacity planning: radix prefix-cache pool + ring suffix at scale
- Level: EXTREME
- Pipeline: cloned-voice TTS + multi-tenant-agent (top commercial workload)
- Axes: capacity:duty, cache:prefix, multi-tenancy:isolation, scale:DC
- Scenario: The top commercial workload (cloned-voice + multi-turn agents) has 86% prefix-cache hits (Fish-S2 L1); capacity planning must size BOTH a shared radix prefix-cache pool AND per-slot ring suffixes.
- System must: Plan hybrid KV (L1: radix/paged prefix-cache for the deterministic ref-audio/system-prompt + ring for the per-utterance suffix), per-tenant-salted prefix entries (G1), size the prefix pool for the working set of distinct ref-audios; admission accounts both the (shared, cache-hit-reducing) prefix cost and the (per-slot) suffix cost — forfeiting the 86% cacheable work if planned ring-only.
- If mishandled: Ring-only capacity plan (§4.3 "prefix sharing ~zero", retired by L1) → recompute 86% cacheable ref-audio KV every request → effective capacity is a fraction of planned for exactly the highest-value workload.

### SCALE-119 — Spill + version-skew correctness across a partially-upgraded cohort
- Level: EXTREME
- Pipeline: AR-TTS, mid-rolling-upgrade
- Axes: spill:kv-migration, deploy:rolling, fault:version-skew, scale:DC
- Scenario: During a rolling upgrade, a stream must spill/migrate but the only free capacity is on a replica running a DIFFERENT model version (v1 source, v2 destination).
- System must: NEVER migrate KV across incompatible versions (different weights/shapes → corrupt) — either spill to a same-version replica, or DON'T migrate (drain the stream on the source to completion), or restart the stream cleanly on v2 (client reconnect); version is part of the migration-compatibility key (like cohort/frame-rate).
- If mishandled: Migrate v1's KV into a v2 slot → shape/weight mismatch → garbage or crash → version-skew corruption during the upgrade (the migration-compatibility blind spot).

### SCALE-120 — The everything-at-once: flash crowd + upgrade + degrade + region-failover + DoS
- Level: EXTREME
- Pipeline: 6 variants, mixed edge+DC, multi-tenant
- Axes: scale:autoscale, burst:flash-crowd, deploy:rolling, fsm:degraded, fault:region-down, multi-tenancy:dos, worker:multi
- Scenario: The worst realistic ops day: a flash crowd hits mid-rolling-upgrade while one region fails over to DC, two boxes are degraded (NPU faults), and one tenant DoSes admission — all simultaneously, fleet-wide.
- System must: Compose every mechanism with the ledger as the single arbiter — FREEZE the rollout (burst), absorb the crowd + region-failover into warm headroom + autoscale (warm-repurpose first), honor degraded replicas' lowered ceilings, rate-limit+quota the DoS tenant at admission's edge, reject true overflow with jittered Retry-After, shed by SLO tier protecting gold, KV-migrate only playback-masked + same-version, never interrupt mid-utterance, fail-closed any gate; the system degrades gracefully (some rejection, some quality-brownout) but never collapses, never leaks, never cross-contaminates tenants, and never drops a live utterance it admitted.
- If mishandled: Any uncoordinated handler (independent admission, no shared ledger, drain-during-burst, migrate-across-version, no per-tenant cap, offline-only gate) turns one bad day into a fleet-wide metastable collapse with leaked slots, cross-tenant contamination, and mass dropped calls.

---

## Coverage

This family covers **120 distinct scaling / multi-tenancy / deployment / lifecycle scenarios** graded SIMPLE (20) → INTERMEDIATE (30) → COMPOUND (30) → EXTREME (40), all grounded in INFER_ENGINE.md §6 (scheduler/admission/duty-ledger) and §8 (config tiers), and the production-failure catalog.

**Config tiers & promotion (§8):** Inline single-stream (SCALE-1), 2nd-stream auto-promotion (SCALE-2), `mode=edge`/`dc` pins (SCALE-3/4), `mode=dc` bs=1 (SCALE-4), lazy ledger spin-up on co-tenant (SCALE-48), one-binary edge↔DC config-scaling with kernel/precision tiering (SCALE-20/56), validated (SCALE-56).

**Autoscaling:** used/total_slots signal (SCALE-5), warm over-provisioning / never-scale-to-zero (SCALE-14), burst within headroom (SCALE-25), warm-capacity-repurposing (SCALE-26/68), reconnect storms (SCALE-82), standing-wave/oscillation damping with hysteresis (SCALE-98/111), region-failover absorption (SCALE-95/120).

**Multi-model co-residency:** admin load/unload (SCALE-8/9), keep-alive TTL/LRU (SCALE-10/54), version swap/hot-swap (SCALE-22/77), load-while-serving OOM via pre-admission reservation (SCALE-21/83/97), 6-variant boxes (SCALE-54/97/107/112).

**Multi-tenancy isolation:** prefix-cache salt + content-hash anti-leak (SCALE-16/90/118), per-tenant quota (SCALE-17/76/96), noisy-neighbor MPS/MIG (SCALE-27/28/55), fairness+aging (SCALE-44/76), one-bad-request HoL (SCALE-29/66), per-SLO tiers (SCALE-57/96/112), priority inversion (SCALE-55/80), DoS at admission edge (SCALE-96/120), bandwidth-fair on unified mem (SCALE-106).

**Lifecycle FSM:** cold-start/model-load (SCALE-6), warmup-gates-readiness first-request cliff (SCALE-7/102/116), calibration per sha×device×driver (SCALE-18/35/45/58/94), Degraded on fault (SCALE-46/67), slot-free multi-trigger + leak control (SCALE-11/92), crash/sidecar/orphan/watchdog (SCALE-40/41/42/43/75/103).

**Rollout:** canary + auto-rollback (SCALE-23/52/113), blue-green (SCALE-24/64/116), drain-on-deploy never-mid-utterance (SCALE-12/79/85), rolling upgrade fleet-wide (SCALE-51/107/119), gate-fail-closed halts rollout incl MOS check (SCALE-70/99), rollback under load (SCALE-113).

**Spill/rebalance:** intra-node first (SCALE-36/71), KV-migration > one frame → playback-buffer-mask (SCALE-37/93/105), version-skew migration guard (SCALE-119), cohort/prefix-locality-aware spill (SCALE-61/71), split-brain ownership (SCALE-101), mid-migration drain race (SCALE-105).

**Heterogeneous fleet & capacity planning:** mixed GPU generations + mixed edge+DC (SCALE-15/63/95/104), per-substrate batch-knee planning (SCALE-104), frame-budget admission across colocated stages (SCALE-19/49/62/114), bottleneck-stage admission (SCALE-34/60), per-substrate compute duty + shared-bandwidth ledger on unified mem (SCALE-38/39/73/86/106), cohort-by-frame-rate incl 12×-spread and dynamic/variable-rate (SCALE-47/69/88/114), idle-slot energy budget (SCALE-117), hybrid-KV planning (SCALE-118).

**Overload (graceful, beyond reject):** reject-don't-glitch baseline (SCALE-13/53), drift→stop-admit→shed hierarchy (SCALE-33/100), deadline-aware/risk-of-violation/brownout (SCALE-89/109/110), metastable damping (SCALE-111), bounded drop-oldest on slow consumer (SCALE-72).

**Fault-isolation at scale:** NaN/illegal-memory blast containment (SCALE-30/91), idle-then-resume + slot-recycle scrub (SCALE-31/32), fan-in deadlock (SCALE-74), crash-storm process isolation (SCALE-103), barge-in reliable cancel (SCALE-50).

**EXTREME headliner (SCALE-81 + SCALE-120):** the 5000-call flash crowd on an autoscaling B200 fleet co-hosting 6 variants with per-tenant SLOs during a rolling upgrade — and the everything-at-once ops-day — exercising the full composition with the ledger as the single arbiter (freeze-rollout, warm-scale, SLO-tier shed, version-safe playback-masked migration, never-mid-utterance, fail-closed gates, no leak/contamination/collapse).
