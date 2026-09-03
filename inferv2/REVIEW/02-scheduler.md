# 02 — WaaV Infer SCHEDULER crate: brutal adversarial review

Scope: `crates/waav-infer-scheduler/src/` (admission, cohort, slot, ring_kv, lease, lifecycle, reconnect, migration, tier, subbucket, gqa, rollout, marker, lib). Static read only.

## Counts

| Severity | Count |
|---|---|
| CRITICAL | 4 |
| HIGH | 9 |
| MED | 11 |
| LOW | 6 |
| **Total** | **30** |

Split: **confirmed** findings are marked `[confirmed]`; **suspected** (needs a runtime/integration trace to nail) are marked `[suspected]`.

## Headline (the one that dwarfs the rest)

The scheduler crate is, at the per-tick level, a **library of pure value-types with exhaustive unit tests but almost no live integration**. The entire layered-admission / lockstep / cohort / KV-firewall / drift-shed / slot-table / ring-KV machinery is **never imported by the live serving path**. The server's real admission (`codec_ar_admission.rs`) imports exactly one symbol — `Ceilings`. The runtime driver *explicitly refuses* to depend on `SlotTable` ("that would be a cycle", `runtime/src/driver.rs:51`). So most of this crate is **shelf-ware**: beautifully-tested, not wired. The control-plane subset (lifecycle FSM, reconnect governor, LoRA/rainbow, migration *start*) **is** wired via `control.rs`. Everything load-bearing for "thousands of concurrent streams under overload" is not. This dominates the enterprise-scale verdict and is filed as CRITICAL-1.

---

## CRITICAL

### [CRITICAL] The whole per-tick admission/scheduling engine is unintegrated shelf-ware
`admission.rs` (entire file) · `slot.rs` · `ring_kv.rs` · `cohort.rs` · `subbucket.rs` · `marker.rs` · `tier.rs` · `lease.rs` · vs `crates/waav-infer-server/src/codec_ar_admission.rs:29` `[confirmed]`

- **What.** `rg 'use waav_infer_scheduler::'` over the whole repo (minus the crate itself) returns: `engine.rs` imports `{Ceilings, CoLoadStage, DutyLedger, RooflineClass, calibrate_co_load_profile}`; `codec_ar_admission.rs` imports `{Ceilings}`; `control.rs` imports `{lifecycle::*, migration::*, reconnect::*, rollout::*, slot::ChannelId}`. **Nothing** imports `Scheduler`, `admit_layered`, `LayeredAdmit`, `Session`/`RiskSlack`, `shed_victim`, `DriftDetector`, `TierArbiter`/`SlaTier`, `KvFirewall`/`KvLengthBudget`, `ThermalState`/`Derate`, `NestedStep`, `MaskedSlotBandwidth`, `SubstrateRoofline`/`BatchKnee`, `CohortAdmission`, `Cohorts`/`StepBuckets`/`SubBuckets`/`VariableStrideLoop`, `SlotTable`, `RingKvCache`, `marker::*`, `gqa::*`, `tier::TierExecutor`, `lease::ComputeLedger`. `runtime/src/driver.rs:8,51,53` states the driver is "scheduler-agnostic … must not depend on `SlotTable`".
- **Why it bites at scale.** The bounded-concurrency + VRAM-accounted + deadline-aware + tier-reserved + drift-shedding admission that this review was asked to validate **is not the admission that runs in production**. The live gate (`codec_ar_admission.rs`) is a separate implementation (semaphore permits + a VRAM cap; `lib.rs:199` `for_serving(permits, box_vram_cap_bytes())`). So every correctness property proven in `admission.rs`'s 3500 lines of tests is moot for the running server. At enterprise scale you are running the *un-reviewed* path; the reviewed path is dead code. This is the single biggest risk in the crate.
- **Fix.** Either (a) wire `Scheduler::admit_layered` / `DutyLedger` / `CohortAdmission` / `SlotTable` into the real request path (replace the semaphore gate), or (b) if the semaphore gate is intentionally the production design, **delete or explicitly quarantine** the unintegrated modules and stop maintaining two admission systems. Until one of those happens, treat the crate's green test suite as *aspirational*, not *operational*. At minimum add a doc/ADR stating which admission system is authoritative.

### [CRITICAL] `DutyLedger::admit` clones the full per-substrate map on every admission call
`admission.rs:821-849` (`let mut projected_compute = self.compute.clone();`) `[confirmed]`

- **What.** Each admit allocates a fresh `HashMap<SubstrateId, f64>` clone of the committed compute map, folds the candidate stages in, then argmaxes. `admit_layered` (`:2444`) and the over-subscribed path (`:2409` `ledger.clone()`) and `admit_tier` (`:1114` `ledger.clone()`) all clone the *entire* ledger per call.
- **Why it bites at scale.** Admission is the hottest control-plane op under a connection storm (thousands of `/connect` per second during a thundering herd / reconnect storm). A per-admit heap allocation + map clone turns the admission path into an allocator-bound bottleneck precisely when the box is already overloaded — the metastable-failure amplifier. With N substrates and a high admit rate it is O(admit_rate · N) allocations/sec of churn on the hot path.
- **Fix.** Project *without* cloning: compute the candidate delta per touched substrate, compare `committed[s] + delta[s]` against the bound in a single pass over the (small) candidate stage set, and argmax incrementally. The map never needs duplication — only the touched substrates' projected sums matter. (Note: this is also moot until CRITICAL-1 is resolved, but if/when the ledger is wired it is a hard cliff.)

### [CRITICAL] No stateful, lock-serialized admission gate — admission has no concurrency story
`admission.rs` (all of `DutyLedger`/`Scheduler`/`CohortAdmission` take `&self`/`&mut self`, no interior `Mutex`) vs `lease.rs:142` (`ComputeLedger` *does* wrap a `Mutex`) `[confirmed]`

- **What.** `DutyLedger::admit` is `&self` (pure projection); `add`/`remove` are `&mut self`. There is no `Mutex`, no atomic, no compare-and-commit. The "admit then `add` only on Admit" protocol (`:815`, `:2368`) is a **read-modify-write across two calls** with the live ledger mutated by a separate `&mut` borrow. The lease module's `ComputeLedger` got this right (single `Mutex`, admit+return same critical section, `lease.rs:128-145`), but the **main** `DutyLedger` did not.
- **Why it bites at scale.** With thousands of concurrent admissions, the `admit` (check) and `add` (commit) are not atomic, so two admissions can each observe headroom, both pass `admit`, then both `add` → **over-admit past the duty bound** (the exact "shed by reject, never over-admit" invariant the crate claims to enforce). The TOCTOU window is the whole point of an admission gate and it is unguarded. (Caller is expected to hold an external lock, but no such contract is encoded or documented, and the live path doesn't use this ledger at all — CRITICAL-1.)
- **Fix.** Give `DutyLedger` a `try_admit_and_commit(&self, stages) -> AdmitDecision` that locks once, projects, and commits atomically on success (mirror `ComputeLedger::try_admit`). Delete the two-phase `admit` then `add` public protocol, or document a hard "caller must serialize" contract.

### [CRITICAL] Migration split-brain "stale-epoch guard" is prose — no dest-side epoch rejection exists
`migration.rs:86-98, 333-359` and `control.rs:715-727` · test `migration.rs:725-739` `[confirmed]`

- **What.** The module docs (`:32-38`) promise: "a monotonic `epoch` prevents split-brain double-admit (SCALE-101): a stale-epoch dest is rejected." But `epoch` is only ever a **stored field** (`OwnershipLease.epoch`, `Migration::start(…, epoch, …)`). There is **no method anywhere** that compares an incoming epoch against a live/registered ownership epoch and rejects the lower one. `grep epoch migration.rs` (non-comment) yields only the field decl, the constructor param, and tests. The "proof" test `monotonic_epoch_prevents_double_admit` asserts only `stale.lease().epoch < live.lease().epoch` — i.e. that `4 < 5`. `control.rs::migrate` (`:715`) passes `epoch` straight into `Migration::start` and never consults a registry.
- **Why it bites at scale.** Under a real network partition + failover (the scenario the epoch exists for), two replicas can each `start` a migration for the same session with un-arbitrated epochs; nothing rejects the stale one, so **both can admit the session** → duplicated streams / cross-user state graft (the SCALE-101 corruption class). The single most safety-critical guard in the migration path is unimplemented.
- **Fix.** Add a per-session ownership registry (`session → live_epoch`) and a `dest_admit(lease) -> Result<(), Stale>` that rejects `lease.epoch < live_epoch` and bumps on accept, under a lock. Wire it into `control.rs::migrate`. Add a test that *actually exercises a rejection*, not a numeric comparison of two literals.

---

## HIGH

### [HIGH] `VariableStrideLoop::run` allocates + sorts a fresh `StepBuckets` BTreeMap per inner step
`subbucket.rs:251-295` (loop body `:271 StepBuckets::group(...)`) `[confirmed]`

- **What.** For each inner step `k in 0..max_nfe`, the loop builds a brand-new `StepBuckets` via `StepBuckets::group(...)` — a `BTreeMap` insert of every still-active slot + a `sort_unstable` + `dedup` + `Vec` collect — purely to extract the single bucket at step `k` (`bucket_for(step)`), which by construction is all the active slots. That is O(max_nfe · n log n) allocation+sort to drive what is a trivial filter.
- **Why it bites at scale.** A DiTAR/CFM cohort with NFE up to ~30 and B up to 64 streams runs ~30 BTreeMap builds per emitted frame *per cohort*, on the inner-solve hot path that must finish inside one frame budget (13–80 ms). This is gratuitous allocator churn on the realtime path and scales with both NFE and B.
- **Fix.** Drop the per-step grouping entirely: at step `k`, the active set is just `slots.filter(|s| nfe_of(s).is_active_at(k))` already in ascending order if `slots` is pre-sorted. No BTreeMap, no re-sort. The "reuse StepBuckets" rationale (`:267`) is a correctness fig-leaf for a one-line filter.

### [HIGH] `NestedSolverReset.reset_order` is an unbounded append-only Vec — leak under churn
`subbucket.rs:592, 647-649` `[confirmed]`

- **What.** Every `reset_slot(slot)` pushes `("inner", slot)` then `("outer", slot)` onto `reset_order`, which is never drained or capped. It is described as a "witness … the scheduler/tests read it" but the production `reset_slot` is the slot-recycle verb fanned out by the DAG transaction.
- **Why it bites at scale.** Slot recycle fires on every stream end / barge-in. At thousands of streams/sec churning over a long-lived replica, `reset_order` grows without bound — a steady memory leak proportional to total lifetime recycles. On the GB10 shared-memory box (see MEMORY: 121 GB shared CPU+GPU pool, OOM-sensitive), an unbounded Vec on the recycle path is exactly the kind of slow leak that eventually OOM-crashes the box.
- **Fix.** Make the reset witness test-only (`#[cfg(test)]`) or a bounded ring/counter. Production recycle must not retain per-recycle history.

### [HIGH] `LifecycleFsm` admission accounting can desync — refcount decrements on `StreamEnded` from any state
`lifecycle.rs:235-238, 250-258, 256-257` `[confirmed/suspected]`

- **What.** `on_admit` increments `refcount` (saturating). `StreamEnded` from a non-Draining state decrements `refcount` (saturating, `:235`). `admit_ok` gates on `refcount < rated_ceiling` (`:257`). But `on_admit` is a separate call from the admit *decision*, and there is no coupling between "the FSM said admit_ok" and "the refcount was incremented" — they can be called in either order or skipped. A `StreamEnded` that arrives without a matching `on_admit` saturates at 0 (silently swallowed); a double `on_admit` over-counts.
- **Why it bites at scale.** `refcount` is the live-stream count that both (a) gates admission against `rated_ceiling` and (b) decides drain completion (`Draining` + refcount==0 → exit, `:227-230`). A desync in either direction is severe: over-count → the replica refuses admissions it could serve (false saturation, capacity starvation) **or** under-count → a drain exits while streams are still live (premature `Failed` → dropped calls). Under churn + partial failure (the exact stress case), mismatched on_admit/StreamEnded pairs accumulate.
- **Fix.** Tie the refcount to an RAII admission ticket (increment on the admit that the FSM approved, decrement exactly once on drop), or make `on(StreamEnded)` idempotent against a tracked set of live channel ids rather than a bare counter. As-is the counter has no integrity guarantee against caller error.

### [HIGH] Drain deadline is never checked against the clock — `DrainDeadline` must be hand-delivered
`lifecycle.rs:151, 221-224, 231-232` `[confirmed]`

- **What.** `Drain` stores `drain_deadline = now + deadline` (`:222-223`), but **nothing in the FSM ever compares `now` against `drain_deadline`**. The only way a drain bounds itself is if the *caller* synthesizes a separate `Event::DrainDeadline` (`:231`). The stored `drain_deadline` field is written and never read (dead field).
- **Why it bites at scale.** The "short-drain-then-abort, never unbounded" guarantee (H7) depends entirely on an external scheduler firing `DrainDeadline` at the right wall-clock moment. If that timer is missed/dropped (control-plane overload, the exact failure mode during mass-drain of a fleet), a stream that never ends keeps the replica `Draining` **forever** — it never frees, never exits, holds its slots/VRAM. The bound is not enforced by the FSM; it is an unverified caller obligation.
- **Fix.** Either check `now >= drain_deadline` inside `on(StreamEnded)` / a new `tick(now)` and auto-transition to `Failed`, or remove the dead `drain_deadline` field and document that the caller owns the timer (and prove the caller actually arms it).

### [HIGH] `DutyLedger` uses default SipHash `HashMap` — keyed on attacker-influenced substrate ids
`admission.rs:704` (`std::collections::HashMap<SubstrateId, f64>`) `[suspected]`

- **What.** `compute` is a `HashMap` with the default `RandomState` (SipHash). Cloned per admit (CRITICAL-2). Keys are `SubstrateId(u8)` — a tiny bounded domain (≤256), so this is not a hash-flood vector, but the SipHash overhead + per-admit clone is pure waste for a ≤N-entry map where N = substrate count (single digits).
- **Why it bites at scale.** For a map of 1–8 entries, a `HashMap` with SipHash + per-call clone is strictly worse than a small `Vec<(SubstrateId, f64)>` or a fixed array indexed by `u8`. On the admit hot path this is measurable overhead with zero benefit.
- **Fix.** Replace with a small fixed-capacity array/`Vec` indexed by substrate id (the domain is `u8` and tiny). Eliminates the hasher cost and makes the projection clone-free (folds into CRITICAL-2's fix).

### [HIGH] `TierExecutor` promotion is a flag flip — `build_batched_machinery` and `adopt` are documented no-ops
`tier.rs:235-254` `[confirmed]`

- **What.** `on_second_stream` "auto-promotes Inline → StageBatched without dropping the first stream." But `build_batched_machinery` (`:235`) only sets `self.batched_machinery_built = true` — it builds **no** stage queues, **no** duty ledger ("The concrete StageQueues / DutyLedger wiring lives in the batcher … here we record that the (lazy) machinery exists"). `adopt` (`:248`) is explicitly a no-op on `live` ("Nothing to mutate on the table"). The whole promotion is a boolean + a tier enum change.
- **Why it bites at scale.** The headline scaling feature — "a single-stream edge replica seamlessly grows into a batched multi-stream replica" — does nothing. When the 2nd stream arrives, no batching machinery is actually instantiated; the executor enum says `StageBatched` but the batched executor is not constructed here. Either the real promotion happens elsewhere (then this type is a misleading stub) or it doesn't happen at all (then 2nd-stream concurrency silently runs on the inline path). Combined with CRITICAL-1 (TierExecutor is not imported by the server), this is untested-in-production growth behavior.
- **Fix.** Either make `build_batched_machinery`/`adopt` actually construct/register the batched executor + duty ledger over the moved `SlotTable`, or collapse the type to honestly reflect that it is just a policy flag and document where the real promotion lives.

### [HIGH] `reconnect` governor admits-without-spending separately from the slot admit — two unsynchronized gates
`reconnect.rs:86-100` + `control.rs:417-421` vs `lifecycle.rs:256` `[suspected]`

- **What.** `ControlPlane::admit_reconnect` (`control.rs:417`) spends a reconnect token, and `admit_ok`/`on_admit` (lifecycle) separately track the rated ceiling. These are two independent gates with no transactional relationship: a reconnect can pass the token bucket but the FSM be at ceiling (or vice-versa), and a token is spent even when the subsequent slot admit fails.
- **Why it bites at scale.** During a reconnect storm, every dialer that passes the rate limiter but then bounces off the rated ceiling has **consumed a reconnect token for nothing** — draining the bucket faster than real admissions, so legitimately-paced reconnects get 429'd by the storm cap while capacity is actually available. Token leakage on the failure path inverts the cap's intent (throttle storms, not steady traffic).
- **Fix.** Spend the reconnect token only after the slot admit succeeds, or refund on admit failure. Make the two gates one ordered transaction (rate-limit → capacity → commit).

### [HIGH] `Cohorts::group` / `StepBuckets::group` / `SubBuckets::group` rebuild a full BTreeMap every tick
`cohort.rs:183-201, 562-580` · `subbucket.rs:397-412` `[confirmed]`

- **What.** Cohort/step/sub-bucket formation allocates a `BTreeMap`, inserts every active slot, then for each key sorts + dedups + collects a `Vec`. `from_active` (`cohort.rs:207`) calls this from the live active set every tick. `compose_nested_inner_solve_per_group` (`cohort.rs:279`) and `run_inner_within_tick` (`subbucket.rs:464`) build *another* BTreeMap for reassembly.
- **Why it bites at scale.** Cohort formation is per-tick (every 13–80 ms) over up to B active slots, and the BTreeMap path is O(n log n) allocation+tree-build *each tick* with no reuse of the prior layout. With thousands of streams across cohorts this is steady per-tick allocator pressure on the realtime loop. The patch-AR path (`PatchClock::regroup_on_boundary`, `cohort.rs:408`) at least gates regroup to patch boundaries, but plain `from_active` does not.
- **Fix.** Cohort membership changes only on admit/free/mask transitions, not every tick — maintain the cohort partition incrementally and only rebuild affected cohorts on a membership delta. Reserve full `group()` for the cold path.

---

## MED

### [MED] `Scheduler::order` clones+sorts the entire active session set every outer tick
`admission.rs:239-243` (`sessions: Vec<Session>` by value, `sort_by_key` allocating the key per element) `[confirmed]`
- **What/why.** `order` takes `Vec<Session>` by value and `sort_by_key(|s| s.order_key(t_f))` recomputes the `RiskSlack` key per comparison-element. For thousands of sessions re-ordered every outer tick this is O(n log n) per tick with a fresh Vec. **Fix.** Sort in place over a persistent buffer; or maintain a priority structure updated on slack change rather than a full re-sort per tick. (Moot under CRITICAL-1 — `order` is unimported.)

### [MED] `SlotTable` membership ops are O(capacity) linear scans — `alloc`/`len`/`is_full`/`exec_mask`
`slot.rs:177-191, 197-216, 318-326` `[confirmed]`
- **What/why.** `alloc` does `iter().position(is_none)` (O(B)); `len`/`is_empty`/`is_full` each scan all B (O(B)); `exec_mask` scans all B per tick. For a single large cohort (B up to 512 on a Wide substrate) and per-tick exec-mask rebuild, this is O(B) per tick + O(B) per admit. **Fix.** Track a free-list (stack of free rows) for O(1) alloc, a live-count for O(1) len/is_full, and build the exec-mask incrementally. (The doc claims B is "thousands of slots"-scale in the prompt; linear scans don't hold there.)

### [MED] `MaskedSlotBandwidth` charges all masked slots one lumped synthetic stage — bandwidth model is coarse
`admission.rs:2178-2182, 2477-2508` `[suspected]`
- **What/why.** The masked-slot bus charge is `masked_count · bytes_per_slot` folded as a single bandwidth-bound synthetic stage. This assumes every masked slot streams identical bytes and that masked traffic serializes uniformly with real stages. Heterogeneous models (different KV footprints per slot — exactly the GQA-native point of the crate) make `bytes_per_slot` a single scalar a lie across a mixed cohort. **Fix.** Charge masked bandwidth per-layout, not a flat per-slot scalar, when a cohort mixes model sizes.

### [MED] `DriftDetector::is_shedding` can release the hold early on a non-monotonic query `now`
`admission.rs:1575-1584` `[confirmed]`
- **What/why.** `observe` enforces monotonic `now` (`:1497`), but `is_shedding(now)` does **not** — it computes `now - tripped_at < hysteresis_secs` against an unchecked `now`. A query with `now < tripped_at` underflows in f64 to a negative (so `< hysteresis` holds → still shedding, safe direction), but a query with a *too-large* `now` (clock jump forward, or a caller passing wall-clock instead of the monotonic source) **releases the hold early**, dropping the shed while drift persists. The doc claims "a `now` earlier than the trip cannot release" but says nothing about a forward jump. **Fix.** Clamp/validate `now` in `is_shedding` or derive it from the same monotonic source `observe` uses.

### [MED] `effective_bound_checked` is O(tiers) per call and `admit_tier` is O(tiers) per admit (reserved_above)
`admission.rs:1054-1060, 1078-1090, 1105-1128` `[confirmed]`
- **What/why.** `reserved_above(tier)` filters+sums over all tiers (O(T)); `effective_bound_checked` sums all reservations again (O(T)). Called per admit. T is small (handful of SLA tiers) so this is minor, but the over-subscribed-tier path additionally `ledger.clone()`s (CRITICAL-2 amplifier). **Fix.** Precompute per-tier effective bounds once at `TierArbiter::new` (tiers are immutable after construction); admit becomes O(1) lookup.

### [MED] `RingKvCache` zero-context degenerate returns `Some(0)` forever — silent mis-attention if reached live
`ring_kv.rs:171-192, 184-187` `[confirmed]`
- **What/why.** A `context == 0` ring's active append returns `Some(0)` and pins the head at 0 (no panic, good), but every append "writes" cell 0 — if a zero-context ring ever reaches the device scatter path it silently aliases all KV onto one cell (the "audio-wrong, no crash" class). It is guarded as "degenerate" but nothing rejects constructing one. **Fix.** Reject `context == 0` at `RingKvCache::new`/`with_layout` (typed error), or assert it is unreachable from any real layout. Don't make silent-aliasing a representable state.

### [MED] `Migration` has no max-in-flight cap — a fault storm spawns unbounded leased buffers
`migration.rs:309-359` + `control.rs:715-727` `[suspected]`
- **What/why.** `Migration::start` mints a `LeasedBuffer` per call with no global cap on concurrent in-flight migrations. A faulting replica spilling all its sessions at once (the FaultSpill scenario) creates one leased buffer + lease per session with no admission bound on the *dest*. **Fix.** Cap concurrent inbound migrations on the dest (a migration admission gate); reject excess with a typed retry. The leased-buffer-per-session is unbounded under mass failover.

### [MED] `ReconnectGovernor::new` clamps a bad rate to `f64::MIN_POSITIVE` → ~infinite retry_after, silent deny-all
`reconnect.rs:66-79, 105-113` `[confirmed]`
- **What/why.** A misconfigured rate (NaN/≤0) clamps to `f64::MIN_POSITIVE`, making `refill_period = 1/rate` overflow to `Duration::MAX` — the governor effectively denies *all* reconnects with a retry-after of ~584 billion years. "Fails closed" is defensible, but a config typo silently bricks reconnect admission for a replica with no loud signal. **Fix.** Reject a non-finite/≤0 rate at construction (typed error) so misconfig is loud at boot, not a silent total-deny at runtime.

### [MED] `RainbowRouter::route` quantizes to 1000 residues — canary fraction granularity floor is 0.1%
`rollout.rs:177, 214-223` `[confirmed]`
- **What/why.** `QUANTUM = 1000`, so `round(fraction * 1000)`: any fraction below `0.0005` rounds to 0 (no canary at all), and the realized fraction is quantized to 0.1% steps. For a fleet doing a 0.01%-canary safety rollout this silently routes zero sessions to canary while reporting a nonzero fraction. **Fix.** Document the 0.1% floor, or raise QUANTUM, or special-case "fraction>0 ⇒ at least the boundary residue" honestly.

### [MED] `from_calibrated` / `calibrate_co_load_profile` build a fresh `DutyLedger` and admit-then-add (over-admit window)
`admission.rs:909-934, 925-932` `[confirmed]`
- **What/why.** Calibration admits the stage set against a fresh ledger then loops `add` — same two-phase non-atomic pattern as CRITICAL-3, here single-threaded at boot so safe in practice, but it reinforces the broken protocol shape that becomes unsafe the moment it is shared. **Fix.** Use the atomic admit+commit from CRITICAL-3's fix here too.

### [MED] `Cohort::contains` / `StepBucket::contains` / `SubBucket::contains` are O(log n) binary search but membership lookups have no index
`cohort.rs:151-153, 527-529` · `subbucket.rs:362-364` `[suspected]`
- **What/why.** Per-slot "which cohort am I in?" requires scanning cohorts then binary-searching each, with no slot→cohort index. For thousands of slots queried per tick this is O(cohorts · log n) per query. **Fix.** Maintain a `slot → cohort_index` map alongside the partition for O(1) reverse lookup.

---

## LOW

### [LOW] `argmax_resource` `.expect(...)` relies on the bus entry always being chained
`admission.rs:773-780` `[confirmed]` — Sound today (bus is always chained), but an `.expect` on the hot path is a latent panic if a future edit removes the chain. Prefer returning `(SharedBandwidth, bandwidth)` as an explicit fallback rather than `max_by(...).expect()`.

### [LOW] `from_throttle_reasons` `.expect(...)` on `Derate::new`
`admission.rs:2028` `[confirmed]` — Provably-valid constants today, but an `.expect` in a `from_*` constructor violates the crate's own "typed-error-not-panic" discipline. Use a `const`-asserted value or a private infallible builder.

### [LOW] `FrameRate::from_hz` saturating-mul can collapse distinct high rates onto `u32::MAX`
`cohort.rs:76-81` `[confirmed]` — `hz.saturating_mul(1000)` means any `hz > ~4.29M` maps to the same `u32::MAX` mHz key → two distinct (absurd) rates collide into one cohort. Doc acknowledges it but calls it "never a silent collision" — it *is* a collision (just of unrealistic inputs). Reject overflow instead of saturating.

### [LOW] `ShedCandidate` shed-victim selection is O(n) `min_by_key` per shed with no structure
`admission.rs:1702-1714` `[confirmed]` — Fine for occasional sheds, but a sustained-overload shed storm re-scans all candidates each shed (O(n) per victim, O(n²) to shed a batch). Consider a victim heap if shedding becomes bursty.

### [LOW] `Session::new` rejects non-finite but accepts arbitrarily negative deadline/cost as "overdue"
`admission.rs:143-155` `[confirmed]` — Intentional (negative = overdue), but a corrupt predictor emitting `-1e300` produces a `RiskSlack` that sorts to one extreme and can starve every other session forever (deadline inversion via a garbage predictor). Consider clamping predicted_remaining to a sane bound.

### [LOW] `lib.rs` re-exports a large flat surface — every internal type is `pub`
`lib.rs:30-52` `[confirmed]` — The crate exports ~70 types at the top level, including pure-internal scheduling primitives. Given most are unintegrated (CRITICAL-1), the broad public surface invites accidental coupling and makes the dead-vs-live boundary invisible to consumers. Narrow the public API to what `control.rs`/`engine.rs` actually use until the rest is wired.

---

## Cross-cutting observations

- **Panic discipline: strong.** Non-test code is essentially `unwrap`/`panic`-free (verified by grep); the few `.expect`s are on provable invariants (LOW-1/2). `#![forbid(unsafe_code)]` (`lib.rs:14`).
- **Determinism: strong.** `f64::total_cmp` everywhere a float is ordered (`RiskSlack`, argmax, tier tightness) — no `partial_cmp` NaN panics. Stable sorts for tie-breaks.
- **RAII ticket discipline: partial.** `ComputeLease`/`LedgerLease` are `#[must_use]` + consume-on-release (good, `lease.rs:60`, `:245`), and `LlmStreamNode::cancel` is idempotent. But a dropped `ComputeLease` that is *not* returned **leaks duty forever** (the `#[must_use]` only warns; there is no `Drop` impl that reclaims) — `lease.rs:63` doc admits "dropping it without returning leaks the reservation." At scale a panic between admit and return leaks duty permanently. Consider a `Drop` that returns to the ledger (needs an `Arc<ComputeLedger>` back-ref like `LedgerLease` has).
- **The lease module is the correctness exemplar** the main `DutyLedger` should have followed: single `Mutex`, admit+return same critical section, consume-to-free. The contrast with `DutyLedger` (CRITICAL-2/3) is stark.

## Top-8 (one line each)

1. **[CRITICAL]** Entire per-tick admission/lockstep engine is unimported shelf-ware; live admission is a separate semaphore path — `admission.rs` whole vs `codec_ar_admission.rs:29`.
2. **[CRITICAL]** `DutyLedger::admit` clones the full substrate map every admit — allocator cliff under storm — `admission.rs:824`.
3. **[CRITICAL]** No lock-serialized admit+commit in `DutyLedger`; two-phase `admit` then `add` is an over-admit TOCTOU — `admission.rs:815,821`.
4. **[CRITICAL]** Migration split-brain "stale-epoch guard" is unimplemented prose; no dest-side epoch rejection exists — `migration.rs:333` / `control.rs:715`.
5. **[HIGH]** `VariableStrideLoop::run` builds a fresh BTreeMap+sort per inner step on the realtime solve path — `subbucket.rs:271`.
6. **[HIGH]** `NestedSolverReset.reset_order` is an unbounded Vec pushed on every slot recycle — leak — `subbucket.rs:647`.
7. **[HIGH]** Drain deadline field is written but never checked vs the clock; bound depends on an external timer that can be lost — `lifecycle.rs:222`.
8. **[HIGH]** `TierExecutor` Inline→StageBatched promotion is a flag flip; `build_batched_machinery`/`adopt` are no-ops — `tier.rs:235,248`.
