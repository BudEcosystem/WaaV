# B13 — Two substrate-agnostic scheduler fixes (pure logic, backend-independent)

Effort B13. Fixes the live `DutyLedger` hazards (item 4) for real + wires priority/fairness into the
live codec-AR admission. All changes are pure control-plane logic (no backend dependency, applies under
any execution backend). Bit-faithful: accepted streams' inference is byte-for-byte unchanged.

## LAW status — GREEN

```
source gb10-env.sh && timeout -k 30 600 cargo test -p waav-infer-scheduler -p waav-infer-server --lib
  waav-infer-scheduler: test result: ok. 142 passed; 0 failed; 0 ignored
  waav-infer-server:    test result: ok. 58 passed; 0 failed; 4 ignored   (4 ignored = pre-existing live-gated)

cargo clippy -p waav-infer-scheduler -p waav-infer-server -- -D warnings
  Finished — CLEAN (0 warnings, 0 errors on both crates)
```

Baseline before changes: scheduler 137 / server 54. Net +5 scheduler, +4 server new tests (one scheduler
test was rewritten in place rather than added — see A3).

Files touched (only the allowed set):
- `crates/waav-infer-scheduler/src/admission.rs` — no-clone admit + `AtomicDutyLedger`
- `crates/waav-infer-scheduler/src/migration.rs` — real dest-side stale-epoch reject
- `crates/waav-infer-scheduler/src/lib.rs` — exports
- `crates/waav-infer-server/src/codec_ar_admission.rs` — priority band + per-tenant fair share

NOT touched (other efforts own them): candle crate, serve.rs, codec_ar_batcher.rs, watchdog.rs, control.rs,
engine.rs. No git commit.

---

## PART A — item 4: the live DutyLedger hazards, FIXED

### A1 — `DutyLedger::admit` no longer clones the full substrate map (CRITICAL-2, the allocator cliff)

`admission.rs` — was `:824 let mut projected_compute = self.compute.clone();` on EVERY admit.

- New private `DutyLedger::projected_argmax(&self, stages, c) -> (ResourceId, f64)` computes the projected
  bottleneck **without cloning** the per-substrate `compute` map:
  1. folds the candidate stages' compute-duty into a tiny scratch `Vec<(SubstrateId, f64)>` delta map —
     `O(stages)` (one session's handful of stages), not `O(committed substrates)`;
  2. argmaxes over every **committed** substrate at `committed[s] + delta[s]` in a single read-only borrow
     of `self.compute` (no copy);
  3. argmaxes over any candidate substrate **absent** from the committed map (a session touching a
     never-loaded substrate is still ranked);
  4. chains the shared bus **last** so an exact compute/bus tie keeps the compute entry — the identical
     stable tie-break as the original `argmax_resource`.
- `admit` now calls `projected_argmax`; the per-admit `HashMap` allocation is gone. The admit is a pure
  read-only feasibility check that allocates only the small candidate-stage scratch.
- **Bit-identical decision proven:** test `no_clone_admit_matches_clone_based_projection_bit_identical`
  reconstructs the OLD clone-based projection as an independent reference and asserts the no-clone `admit`
  names the identical `(resource, duty, bound)` / Admit across the cases the argmax must get right: a
  touched-substrate reject, a reject on a substrate **absent** from the committed map, a **bus** reject
  with all compute free, and the `== S` boundary.

### A2 — atomic `admit_and_commit` (CRITICAL-3, the admit→add TOCTOU)

The two-phase `admit` (`&self` check) then `add` (`&mut self` commit) is a read-modify-write across two
calls — two concurrent admits can both pass then both commit and over-admit past the duty bound.

- New `pub struct AtomicDutyLedger { inner: Mutex<DutyLedger> }` — wraps the value-type ledger under ONE
  `Mutex`, **mirroring the `lease.rs::ComputeLedger` pattern the review praised** (admit + reserve in one
  critical section, consume-to-free).
- `admit_and_commit(&self, stages, c) -> AdmitDecision`: under a single lock hold, runs the (clone-free)
  feasibility projection AND — iff every resource ≤ S — commits the reservation (`add`) before releasing
  the lock. An `Admit` has **already** committed (no separate `add` for the caller to forget — the source
  of the race); a `Reject` leaves the ledger unmutated. `release(&self, stages, c)` is the symmetric
  consume-to-free.
- The read-only `admit` / `&mut add` / `Clone` / `Default` on the plain `DutyLedger` are **kept** for the
  ~30 single-threaded callers that only probe (boot calibration, per-tier / per-layer projections, the
  engine's measured-bus snapshot) — `admit`'s doc now states the TOCTOU contract and points concurrent
  committers at `AtomicDutyLedger`.
- **Wrapper, not interior mutability on `DutyLedger`:** the value type is shared by many callers that need
  `&mut add` / `Clone`; only the concurrent admit path must serialize, so a thin `Mutex` wrapper is the
  least-invasive shape (and exactly the `lease.rs` shape).
- **Live `engine.rs::admit_bandwidth` caller checked — no rewiring needed.** Read-confirmed: the live
  `admit_bandwidth` (`engine.rs:712`) only **reads** `ledger.bandwidth_utilization()` against a constant
  threshold — it is a read-only probe of the committed measured-bus profile, it does NOT admit-then-commit
  on a shared ledger. The two-phase `admit`/`add` commit exists only in `calibrate_co_load_profile`
  (boot-time, single-threaded, safe). So the task's conditional ("wire the caller to the atomic path **if
  it commits**") resolves to: it does not commit, so no change to the caller; `AtomicDutyLedger` is the
  correct primitive now available for any future concurrent committing admitter.
- **Tests:**
  - `atomic_admit_and_commit_is_one_critical_section` — an Admit committed the duty in-lock; a Reject
    committed nothing; `release` returns it.
  - `concurrent_admit_and_commit_never_over_admits` — 64 threads each reserve 0.10 duty against S=0.8;
    **exactly 8** admit, committed Σ lands **exactly at S**, never 9 (the over-admit the two-phase TOCTOU
    permitted). This test fails against the old `admit`-then-`add`.
  - `atomic_ledger_seeds_from_a_committed_profile` — `new()` seeds from a boot-calibrated profile; the
    atomic admit reserves on top and the bus-saturation reject stays typed.

### A3 — migration dest-side stale-epoch reject (CRITICAL-4, the SCALE-101 split-brain)

`migration.rs` — the `epoch` was only a stored field; there was NO method comparing an incoming epoch
against a live ownership epoch and rejecting the lower one. The "proof" test only asserted `4 < 5`.

- New `pub struct OwnershipRegistry { live: Mutex<HashMap<ChannelId, u64>> }` — the dest-side
  `session -> live_epoch` registry the module docs always *promised* but never implemented.
- `dest_admit(&self, lease: &OwnershipLease) -> Result<(), StaleEpochReject>`: under ONE lock hold,
  admits the transferred lease **iff** `lease.epoch >= live_epoch` and **bumps** the live epoch on accept;
  a strictly-lower epoch is a **typed** `StaleEpochReject { session, stale, live }` and the registry is
  left unmutated. A never-seen session is admitted (it establishes the epoch). The compare-then-bump is
  one critical section (the `ComputeLedger`/`AtomicDutyLedger` discipline) so concurrent dest admits for
  one session cannot both pass.
- `StaleEpochReject` is a **dedicated** typed error (NOT a new `MigrationReject` variant) — deliberately,
  so the existing exhaustive `match e` over `MigrationReject` in the untouchable `control.rs:725` stays
  exhaustive and compiles unchanged. It is raised at a different gate (dest admission) than
  `MigrationReject` (migration *start*), so the separation is also semantically correct.
- **Test (rewritten to exercise a REAL rejection, per the synthesis directive):**
  `monotonic_epoch_prevents_double_admit` now: admits the live epoch 5 → registers it → the stale epoch 4
  replay is a typed `StaleEpochReject{session, stale:4, live:5}` and does NOT bump the live epoch →
  re-admitting epoch 5 is idempotent (`>=`) → a newer epoch 6 admits and advances the owner → the former
  epoch-5 owner is then itself stale and fenced.
- **Added** `dest_admit_isolates_sessions_and_serializes_concurrent_admits` — distinct sessions arbitrate
  independently (a fence on one never touches another); 20 threads racing dest-admits for ONE session at
  epochs 1..20 converge to the max epoch (20) as the sole owner — never a state where two epochs both own
  the session.

---

## PART B — priority + fairness wired into the LIVE codec-AR admission

`server/codec_ar_admission.rs` was FCFS bounded-gate only (concurrency CAS + VRAM CAS + deadline). Added the
two vLLM "real scheduler" control-plane dimensions ON TOP of the proven gate (the bounded + VRAM + deadline
checks are unchanged — additive, not a rewrite).

### New types
- `Priority { Low, Normal (default), High }` — the per-request ordering tier.
- `TenantId(Arc<str>)` — the per-tenant / per-voice fair-share key; `anonymous()` is the shared bucket.

### Priority band
- The top `reserved_high_slots` (default 2, clamped `< max_inflight`) of the concurrency budget are
  reserved for `Priority::High`. A `Normal`/`Low` request's effective cap is `max_inflight −
  reserved_high_slots`; a `High` request may use the **whole** `max_inflight`. So once the non-reserved
  band is full, `Normal`/`Low` shed (typed `AdmissionRejected`) while `High` still admits into the reserved
  band — high priority is admitted ahead of low under saturation. Same lock-free CAS as before; the global
  bound is still never exceeded.

### Per-tenant fair share
- Each non-anonymous tenant may hold at most `⌈max_inflight × share⌉` (default share 0.5) in-flight streams;
  a request from a tenant at its cap is shed **even with global headroom**, so one noisy tenant/voice
  cannot monopolize the box and a second tenant is never starved.
- Accounting is `Mutex<HashMap<TenantId, usize>>` (tenant ids are a dynamic unbounded key domain; admission
  is a once-per-stream control-plane op, not the per-frame hot path; lock never held across `.await`; entry
  removed at 0 so the map never grows under tenant churn). The `AdmissionTicket` carries its `TenantId` and
  decrements the count (RAII) on drop — symmetric to the admit bump; a post-reserve shed (deadline/VRAM)
  releases the tenant slot too.

### Backward compatibility / bit-faithfulness
- The existing `try_admit(deadline)` signature (which the untouchable `codec_ar_batcher.rs:170` calls) is
  **preserved** and now delegates to a new `try_admit_prioritized(deadline, priority, tenant)` as
  `Priority::High` + `TenantId::anonymous()`. High ⇒ not relegated by the band; anonymous ⇒ exempt from the
  per-tenant cap. Result: the legacy path admits the **full** `max_inflight` with no per-tenant cap —
  **identical capacity and behavior** to the pre-priority gate, so the live codec-AR batcher path and its
  bit-identity are unchanged. Priority/fairness engage only when a caller passes an explicit tier/tenant
  via `try_admit_prioritized`.
- `new(...)` delegates to a new `with_fairness(..., reserved_high_slots, tenant_share)` builder (the
  parameterized seam tests drive); `for_serving` is unchanged.

### Tests
- `legacy_try_admit_still_uses_full_budget_no_tenant_cap` — even with reserved_high=3 + share=0.25, the
  legacy `try_admit` fills the whole budget (8/8) and sheds the 9th on the hard bound (bit-faithful bound).
- `high_priority_admitted_ahead_of_low_under_saturation` — band 2 of 4: two Normal fill the non-reserved
  band, a third Normal AND a Low are shed, but `High` is admitted into the reserved band at the same
  saturation; at the global bound even High sheds (bound never exceeded).
- `no_tenant_exceeds_its_fair_share` — cap = ⌈8×0.5⌉ = 4: a noisy tenant fills 4 then is shed (typed,
  "fair share") with 4 global slots free; a different tenant is not starved; an RAII drop reopens one of the
  tenant's share.
- `later_gate_shed_releases_tenant_and_global_counters` — a VRAM-leg shed rolls back BOTH the global count
  and the tenant count (the fairness ledger never leaks).
- `concurrent_tenant_admits_never_exceed_fair_share` — 64 threads from one tenant, cap = ⌈64×0.25⌉ = 16:
  exactly 16 admit, the live tenant count is exactly 16, never over (the Mutex serializes check+bump).

---

## Summary

| Fix | Status |
|---|---|
| A1 no-clone `admit` (CRITICAL-2) | DONE — clone-free `projected_argmax`, bit-identical decision proven |
| A2 atomic `admit_and_commit` (CRITICAL-3) | DONE — `AtomicDutyLedger`, one critical section; concurrent-no-over-admit proven; live `admit_bandwidth` confirmed read-only (no rewire needed) |
| A3 migration dest-side epoch reject (CRITICAL-4) | DONE — `OwnershipRegistry::dest_admit`, typed `StaleEpochReject`, real rejection + concurrent convergence tested |
| B priority band + per-tenant fair share | DONE — added to the live gate; high-ahead-under-saturation + no-tenant-monopoly proven; legacy path bit-faithful |

LAW: tests GREEN (scheduler 142/0, server 58/0), clippy CLEAN, bit-faithful (accepted streams identical),
only the allowed files touched, no git commit.
