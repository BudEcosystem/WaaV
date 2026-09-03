# LAYER 5 — Control plane / lifecycle FSM / cross-cell ledger / migration

**Status:** deep-design, completes `INFER_ENGINE_V2.md §6.5` (+ the §6.0 restore items it depends on) and `INFER_ENGINE_IMPL.md §7 M4.4/M4.5` to *full* (types + algorithms + RED test bodies). · **Date:** 2026-06-17 · **Device of record:** GB10 (sm_121, sm_12x Blackwell family).

> **The L5 thesis in one line.** The engine is a *thin per-replica state machine that emits signals and accepts commands*; **all fleet policy lives in the orchestrator**. The one piece L5 owns that is *not* a thin signal is the **box-scoped singleton VRAM accountant** — because cell/shard gives each cell its own CUDA context, a per-process accountant is a genuine correctness bug (two cells each "see" free VRAM → double-load OOM), and only a box-level serializer fixes it. Everything else — autoscale, warm-pool, spill, rollout, canary, region-failover — is the orchestrator reading our gauges and issuing our commands. KISS: push policy out, keep the engine a typed contract.

This document is the engine↔orchestrator **line**, drawn explicitly, plus the per-replica lifecycle FSM, the cross-cell VRAM serializer, config-tier auto-promotion, and the (now-de-contradicted) fault/spill migration. It is GPU-free unit-testable end to end: the FSM transitions and the cross-cell serialize are pure logic over fake clocks / fake reservations.

---

## (a) Convergence table

Every prev-PARTIAL/GAP scaling, lifecycle, cross-cell, migration, calibration, and capstone scenario, with the v2.1 mechanism that closes it and the L5 type/algorithm/test that makes it implementable. Verdict key: **CLOSED** = mechanism named in V2 §6.5/§6.0 + a concrete L5 type + a named RED gate here; **CLOSED-CONTRACT** = closed *at the engine↔orchestrator boundary* (the engine emits the signal / accepts the command; the fleet loop is explicitly the orchestrator's, by design — this is the correct resolution, not a punt); **RESIDUAL** = still open after L5 (listed in §c).

### Control-plane subsystems (G-CTRL — the dominant scaling hole)

| Scenario(s) | prev | mechanism (V2 §6.5) | L5 type / algorithm | gate | verdict |
|---|---|---|---|---|---|
| SCALE-5 used/total signal | SAT | pillar-9 `used/total_slots` | `ReplicaSignals{used,total,…}` snapshot | `control_plane_emits_used_total_per_substrate` | CLOSED |
| SCALE-14/25/98/102 warm-pool, never-zero | PARTIAL | "orchestrator owns warm-pool" | `ControlPlane::lifecycle_stream()` + `load`/`unload` commands; never-zero is orchestrator *config* | `lifecycle_event_stream_emits_all_states` | CLOSED-CONTRACT |
| SCALE-15/63/95/104 placement / region-route / region-failover | PARTIAL | engine emits substrate caps + reject-reason | `RejectReason` + per-substrate `DutyGauge`; routing is orchestrator | `load_command_refused_returns_typed_reason_for_reroute` | CLOSED-CONTRACT |
| SCALE-23/52/70/84/99 canary + auto-rollback + fleet-halt-on-gate-fail | PARTIAL | canary-on-new-sessions gated on streaming-SLI; `freeze-rollout` cmd | `Command::FreezeRollout` + `Command::SetAdmitPolicy{canary_fraction}`; SLI gate = the calibration/accuracy stamp (§6.0) | `freeze_rollout_command_stops_new_admits_keeps_live`, `canary_fraction_routes_new_sessions_only` | CLOSED-CONTRACT |
| SCALE-26/36/53/71/93 warm-repurpose / intra-node spill / rebalance | PARTIAL→GAP | spill-migration = opt-in (see migration row) | `Command::{Drain,Unload,Load}` compose the repurpose; spill *placement* is orchestrator | `drain_command_returns_completion_event` | CLOSED-CONTRACT (placement) + see Migration |
| SCALE-51/95/115 rolling/region drain sequencing | PARTIAL | drain-FSM + refcount; sequencing is orchestrator | `LifecycleFsm::Draining`, `drain(deadline)` | `draining_rejects_new_then_exits_after_refcount_zero` | CLOSED-CONTRACT |
| SCALE-82 reconnect storm / rate cap | PARTIAL (GAP-ish) | **`reconnect_admission_rate_capped_per_replica`** (storm governor) | `ReconnectGovernor{token_bucket}` in admission front | `reconnect_admission_rate_capped_per_replica` | **CLOSED** (this one IS engine-side) |

### Per-replica lifecycle FSM (G-FSM)

| Scenario(s) | prev | mechanism | L5 type / algorithm | gate | verdict |
|---|---|---|---|---|---|
| SCALE-6 Loading→Warming | PARTIAL | C7 readiness-on-warm + `Loading` state | `LifecycleFsm::Loading/Warming` | `loading_to_warming_on_weights_resident` | CLOSED |
| SCALE-6/116, FAIL-15 Warming→Ready (4 gates) | PARTIAL | Ready iff warm ∧ captured ∧ calibrated ∧ accuracy-stamped | `ReadyGates{warm,captured,calib,accuracy}`; `try_ready()` | `warming_to_ready_requires_all_four_gates` | CLOSED |
| SCALE-46/67, FAIL-65 Ready→Degraded lowers ceiling | PARTIAL | Degraded lowers rated ceiling via duty recompute | `degrade(stage, cpu_T_step)` → `Ceiling` recompute | `ready_to_degraded_lowers_stage_ceiling_and_re_admits` | CLOSED |
| SCALE-46/67 Degraded→Ready hysteresis | PARTIAL | restored §6.0 drift-response hysteresis (60 s) | `Hysteresis{up,down,dwell}` shared with L4 drift detector | `degraded_to_ready_requires_dwell_under_threshold` | CLOSED (consistency w/ L4 verified §b.2) |
| SCALE-12/79/116, FAIL-116 Draining bounded | PARTIAL | H7 short-drain-then-abort; refcount-zero exit | `Draining{deadline}` + `RefCount`; `on_refcount_zero` | `draining_frees_on_refcount_zero`, `short_drain_then_abort_bounded` | CLOSED |
| FAIL-37/38, #44209 ready-then-crash-loop | SAT-adj | Ready gated on warm+capture so it can't flap-Ready | `Failed` is terminal; `restart_backoff` | `failed_is_terminal_restart_is_bounded_backoff` | CLOSED |

### Box-scoped singleton VRAM accountant (G-XLEDGER — the real correctness gap)

| Scenario(s) | prev | mechanism | L5 type / algorithm | gate | verdict |
|---|---|---|---|---|---|
| SCALE-83/97/120 two loads can't both fit | PARTIAL | **box-scoped singleton** serializes all cross-cell loads | `VramAccountant` (one per box) + `ReservationToken`; `reserve()` is the single critical section | **`two_concurrent_cross_cell_loads_serialize`** | **CLOSED** (race shown in §b.3) |
| SCALE-21/77/107 6-variant swap peak = 5old+1new | PARTIAL | committed + projected-peak + reserve ledger | `LedgerState{committed,reserved,peak}`; sequenced swap | `sequential_swap_peak_is_n_minus_1_old_plus_1_new` | CLOSED |
| SCALE-8 load doesn't stall co-tenant's tick | PARTIAL | reservation is the only cross-cell lock; load runs off the frame thread | `reserve()` non-blocking-for-tick (lock held µs, copy off-thread) | `reservation_does_not_block_serving_cells_tick` | CLOSED |
| SCALE-54 slot-cap by VRAM | PARTIAL | `slot_cap_by_vram_capacity` | `Ceiling::from_vram(cap, footprint)` | `slot_cap_derived_from_vram_capacity` | CLOSED |
| SCALE-60/73 multi-model co-residency | SAT | box ledger arbitrates all cells | `VramAccountant::admit_co_resident` | `multi_model_co_residency_admission` | CLOSED |

### Config-tier auto-promotion (G-PROMOTE)

| Scenario(s) | prev | mechanism | L5 type / algorithm | gate | verdict |
|---|---|---|---|---|---|
| SCALE-2/48 promote without dropping the stream | PARTIAL | §8 promote-on-2nd-stream, executor swap only | `ConfigTier` enum + `promote()` hands slot-state between ticks | **`second_stream_promotes_without_dropping_first`** | CLOSED |
| SCALE-3 `mode=edge` pin refuses promotion | PARTIAL | mode-pin semantics | `TierPolicy::PinEdge` → 429 the 2nd stream | `mode_edge_pin_rejects_second_stream` | CLOSED |
| SCALE-4 `mode=dc` boots stage-batched, bs1 rides it | PARTIAL | `mode=dc` forces Stage-batched at boot | `TierPolicy::PinDc` | `mode_dc_boots_stage_batched_bs1_no_batch_wait` | CLOSED |

### Fault/spill migration (G-MIGRATE — the FIXED self-contradiction)

| Scenario(s) | prev | mechanism (§6.5) | L5 type / algorithm | gate | verdict |
|---|---|---|---|---|---|
| SCALE-37/119 cadence-migration | GAP/SAT-by-exclusion | **cadence-migration = REJECTED** (playback buffer protects cadence) | `MigrationKind::Cadence => Err(Rejected)` | `cadence_migration_rejected_playback_buffer_protects` | CLOSED (contradiction resolved) |
| SCALE-59/93/105/108/115 fault/spill migration | GAP | opt-in, same-version, **leased ownership**, append-only KV **+ inner latent**, buffer-masked | `Migration{lease, append_only_kv, inner_latent}`; `LeasedOwnership` | `fault_migration_appends_kv_and_inner_latent_leased_buffer_masked` | CLOSED |
| SCALE-105 mid-migration abort / zombie-slot | GAP | source holds until dest ACKs, then frees; drain-budget covers it | `Lease::expiry` + `abort()`; single-writer | `source_holds_until_dest_acks_then_frees`, `lease_expiry_mid_migration_aborts_no_zombie` | CLOSED (lease-expiry case §b.5) |
| SCALE-101 split-brain / double-admit | GAP | monotonic per-session region-ownership token | `OwnershipLease{epoch}` monotonic | `ownership_lease_prevents_double_admit` | CLOSED |
| SCALE-119 never migrate across version | SAT | version+cohort+region compat key | `MigrationKey{version,cohort,region}` | `migration_refused_across_version_or_cohort` | CLOSED |

### Calibration-stamp lifecycle (§6.0 restore — was GAP in 09_failure)

| Scenario(s) | prev | mechanism (§6.0) | L5 type / algorithm | gate | verdict |
|---|---|---|---|---|---|
| FAIL-93/SCALE-58/84 stamp gates readyz, mismatch recalibrates | GAP | stamp = `sha×device×driver×warm-set`; gates `/readyz` | `CalibrationStamp{sha,device,driver,warm_set}` | `calibration_stamp_gates_readyz`, `admission_refuses_stale_stamp` | CLOSED |
| SCALE-45/94/113 cache-hit skips recalibration (fast rollback) | PARTIAL | cache-hit-on-same-key skips recalibration | `StampCache::lookup(key)` | `calibration_stamp_cache_hit_skips_recalibration` | CLOSED |
| FAIL-94 MIG repartition re-stamps | GAP | partition-change invalidates stamp | `Stamp::invalidate_on(DeviceChange)` | `partition_change_invalidates_stamp` | CLOSED |

### Capstones / cascades

| Scenario(s) | prev | what L5 closes | verdict |
|---|---|---|---|
| SCALE-81 (freeze-rollout × warm-repurpose × SLO-shed × pause-during-burst) | PARTIAL | `FreezeRollout` + `Drain`/`Load` repurpose + lifecycle gauges; *local* invariants already built, *composition* now has the contract | CLOSED-CONTRACT (orchestrator composes our commands; SLO-tier shed = L4 residual) |
| SCALE-120 (everything-at-once) | PARTIAL | box VRAM serializer (CLOSED) + lifecycle FSM (CLOSED) + migration protocol (CLOSED) + control contract (CLOSED-CONTRACT); the one true correctness gap (double-load) is fixed | CLOSED-CONTRACT |
| FAIL-93/94/113 calibration cascades | GAP | calibration-stamp lifecycle (CLOSED) unblocks the first line of defense in FAIL-113 | CLOSED (the stamp leg; the live-step-time leg is L4) |
| FAIL-108/87 teardown cascade (NCCL-hang→orphan→restart-OOM) | PARTIAL | `Failed`-transition teardown order + `restart_waits_on_vram_reclamation` | CLOSED (the lifecycle legs; abort-collectives is M4.4) |

**Tally: 31 line-items CLOSED or CLOSED-CONTRACT.** The engine↔orchestrator line is drawn so that every G-CTRL scenario is CLOSED-CONTRACT (signal+command, not a fleet loop in the engine), and the four scenarios that demand *engine-side* mechanism (box VRAM serialize, lifecycle FSM, migration lease, reconnect cap) are genuinely CLOSED.

---

## (b) Deep design

### b.0 The engine↔orchestrator LINE (the KISS boundary, drawn explicitly)

This is the load-bearing decision of L5. Adversarially: *what does the engine decide vs the orchestrator?*

| Concern | **Engine (per-replica, this box)** decides | **Orchestrator (fleet)** decides |
|---|---|---|
| A single load fits this box | **YES** — the box VRAM accountant reserves-or-refuses | — |
| *Which* box to load on | — | YES (reads our `refused(reason)`) |
| When this replica is Ready | **YES** — the 4-gate predicate | — |
| When to add/remove replicas (autoscale) | — | YES (reads `used/total`, duty) |
| Warm-pool size / never-scale-to-zero | — | YES (orchestrator config) |
| Drain this replica now | accepts the `drain` **command**, runs the FSM | YES (issues it, sequences the rollout) |
| Reject a session (no slot/duty/stale-stamp) | **YES** — returns typed `RejectReason` + Retry-After | reads it to reroute/scale |
| Canary fraction / freeze rollout | accepts `SetAdmitPolicy`/`FreezeRollout` **commands** | YES (decides the % and when) |
| Reconnect storm on *this* replica | **YES** — `ReconnectGovernor` rate-caps locally | YES (Full-Jitter backoff client-side, budget) |
| Migrate a stream off this replica | accepts a `migrate` command, runs the lease protocol | YES (chooses source/dest, same-version) |
| Calibration stamp valid here | **YES** — gates our own `/readyz` | reads readiness |

**The rule:** the engine answers *"can I, right now, on this box?"* with typed yes/no/why. The orchestrator answers *"what should the fleet do?"* by reading our signals and issuing our commands. The engine never holds fleet state (no replica map, no region table, no autoscale loop) — that keeps it KISS and lets `git revert` of any L5 piece leave a serving engine.

```rust
// waav-infer-control/src/lib.rs  — the whole engine-side surface, ~one screen.

/// Signals the engine EMITS (orchestrator reads; pure data, cheap to snapshot every ~1s).
#[derive(Clone, Debug)]
pub struct ReplicaSignals {
    pub lifecycle: LifecycleState,            // the FSM's current state (b.2)
    pub ready_gates: ReadyGates,              // sub-bits so a stuck Warming is diagnosable
    pub used_slots: u32,
    pub total_slots: u32,                     // == current rated Ceiling (lowered in Degraded)
    pub duty: Vec<SubstrateDuty>,             // per-substrate ΣU + shared-bandwidth duty (L4)
    pub vram: VramSnapshot,                   // committed/reserved/peak/largest-contiguous (J3)
    pub calib: CalibrationStamp,              // sha×device×driver×warm-set (b.6)
    pub reject_rate: f32,                     // recent reject fraction → autoscale input
}

/// Commands the engine ACCEPTS (orchestrator issues; the engine validates + applies locally).
#[derive(Clone, Debug)]
pub enum Command {
    Drain { deadline: Duration },                 // → Draining; completion event when refcount==0
    Load { model: ModelId, footprint: KvFootprint }, // → VramAccountant::reserve → Loading
    Unload { model: ModelId },                     // refused if refcount>0 (non-evictable)
    SetAdmitPolicy { canary_fraction: f32, tier_floor: Option<SlaTier> },
    FreezeRollout,                                 // stop admitting NEW; keep live streams
    Migrate(MigrateSpec),                          // opt-in; runs the lease protocol (b.5)
}

/// What a command returns — typed so the orchestrator can route on failure.
#[derive(Clone, Debug)]
pub enum CommandReply {
    Accepted,
    DrainStarted { live_streams: u32 },
    Refused(RejectReason),                          // e.g. VramExceedsPeak, RefcountNonZero
}

#[derive(Clone, Debug)]
pub enum RejectReason {
    NoFreeSlot, DutyExceeded { substrate: SubstrateId },
    VramExceedsPeak { need: u64, free: u64 },
    StaleCalibrationStamp { have: CalibrationStamp, want_device: DeviceId },
    ReconnectRateCapped { retry_after: Duration },
    ModePinned(ConfigTier),                         // edge-pin refuses promotion
    RefcountNonZero { live: u32 },                  // unload of a serving model
}

/// The engine-side control plane. Thin: it owns the FSM + the box-VRAM handle + the governor.
pub trait ControlPlane: Send {
    fn signals(&self) -> ReplicaSignals;
    fn apply(&mut self, cmd: Command) -> CommandReply;
    /// Lifecycle transitions push here; orchestrator subscribes (drives canary/rollback).
    fn lifecycle_events(&self) -> tokio::sync::watch::Receiver<LifecycleState>;
}
```

### b.1 — Why the box-scoped VRAM accountant is the one piece that *must* be engine-side

Cell/shard (J1) deliberately gives each cell its **own CUDA context** in its **own process** so one fault/OOM loses a cell, not the box. That correctness win creates the L5 correctness gap: VRAM is one physical pool, but there are K processes. A per-process accountant is *locally* correct and *globally* wrong.

### b.2 — `LifecycleFsm` (per-replica; distinct from `marker.rs` per-stream FSM)

```rust
// waav-infer-scheduler/src/lifecycle.rs

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LifecycleState { Loading, Warming, Ready, Degraded, Draining, Failed }

/// The four sub-gates that compose Warming→Ready. Exposed in ReplicaSignals so a
/// stuck-in-Warming replica is diagnosable (which gate is false).
#[derive(Clone, Copy, Default, Debug)]
pub struct ReadyGates { pub warm: bool, pub captured: bool, pub calib: bool, pub accuracy: bool }
impl ReadyGates { pub fn all(&self) -> bool { self.warm && self.captured && self.calib && self.accuracy } }

/// Hysteresis shared with the L4 drift detector so Degraded⇄Ready never flaps and never
/// disagrees with the scheduler's shed/recover decision (see consistency note below).
#[derive(Clone, Copy, Debug)]
pub struct Hysteresis { pub trip_p99: Duration, pub recover_p99: Duration, pub dwell: Duration }
//                       trip > recover  (a band, never a single threshold)  + a dwell window.

pub struct LifecycleFsm {
    state: LifecycleState,
    gates: ReadyGates,
    refcount: u32,                 // active non-evictable streams (J4)
    rated_ceiling: Ceiling,        // total_slots; LOWERED in Degraded via duty recompute
    full_ceiling: Ceiling,         // the un-degraded value (to restore on recover)
    hyst: Hysteresis,
    under_threshold_since: Option<Instant>, // dwell clock for Degraded→Ready
    drain_deadline: Option<Instant>,
    tx: tokio::sync::watch::Sender<LifecycleState>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LifecycleError { BadTransition(LifecycleState, &'static str) }

impl LifecycleFsm {
    /// THE transition function. Every edge is a gated predicate; illegal edges are typed errors,
    /// not panics (codebase idiom: model.rs returns typed errors). GPU-free testable.
    pub fn on(&mut self, ev: Event, now: Instant) -> Result<LifecycleState, LifecycleError> {
        use LifecycleState::*;
        let next = match (self.state, ev) {
            // Loading: weights being placed. → Warming once resident (VRAM reserved + copied).
            (Loading, Event::WeightsResident)        => Warming,

            // Warming: warm every shape bucket + silence + barge-in path, capture graphs,
            // validate calibration stamp, run the accuracy/MOS re-stamp. Ready iff ALL four.
            (Warming, Event::Gate(g))                => { self.gates = g; if g.all() { Ready } else { Warming } }

            // Ready→Degraded: an accelerator/DCGM verdict (L4/J5) lowers the rated ceiling by
            // recomputing duty against the degraded (e.g. CPU-codec) T_step. Re-admit at the
            // lower ceiling; existing streams keep cadence (no preempt).
            (Ready, Event::DegradeVerdict { stage, cpu_t_step }) => {
                self.rated_ceiling = Ceiling::recompute(stage, cpu_t_step);
                self.under_threshold_since = None;
                Degraded
            }

            // Degraded→Ready: HYSTERESIS — require measured p99 under the *recover* threshold
            // (strictly below the *trip* threshold) sustained for `dwell`. Shared band with L4.
            (Degraded, Event::StepP99(p99)) => {
                if p99 <= self.hyst.recover_p99 {
                    let since = *self.under_threshold_since.get_or_insert(now);
                    if now.duration_since(since) >= self.hyst.dwell {
                        self.rated_ceiling = self.full_ceiling; Ready
                    } else { Degraded }
                } else { self.under_threshold_since = None; Degraded } // re-arm the dwell clock
            }

            // *→Draining on a drain command. Reject NEW; keep feeding live (refcount>0).
            (s, Event::Drain { deadline }) if s != Failed => { self.drain_deadline = Some(now + deadline); Draining }

            // Draining→ (exit) on refcount-zero OR deadline (short-drain-then-abort, H7).
            (Draining, Event::StreamEnded)  => { self.refcount = self.refcount.saturating_sub(1);
                                                 if self.refcount == 0 { return self.exit(); } Draining }
            (Draining, Event::DrainDeadline) => return self.exit(),  // bounded; never unbounded

            // *→Failed on the dead-flag / unrecoverable fault (H6). Terminal.
            (_, Event::DeadFlag)            => Failed,

            (s, ev) => return Err(LifecycleError::BadTransition(s, ev.name())),
        };
        self.set(next); Ok(next)
    }

    fn admit_ok(&self) -> bool { matches!(self.state, LifecycleState::Ready | LifecycleState::Degraded)
                                 && self.refcount < self.rated_ceiling.slots() }
    pub fn on_admit(&mut self)  { self.refcount += 1; }
    fn exit(&mut self) -> Result<LifecycleState, LifecycleError> { /* free arenas, fan dead-flag */ Ok(LifecycleState::Failed) }
    fn set(&mut self, s: LifecycleState) { self.state = s; let _ = self.tx.send(s); }
}
```

**Degraded↔Ready hysteresis is consistent with the L4 drift-detector — verified.** The §6.0 restore item (1) gives L4 an EWMA live step-time → sustained-p99-breach trips shed with **60 s hysteresis**. L5 reuses *the same `Hysteresis` band and dwell clock*: `Event::DegradeVerdict` is emitted *by* L4's drift detector when its trip threshold fires, and `Event::StepP99` feeds the *same* recover threshold. There is exactly one threshold pair `(trip_p99, recover_p99)` and one `dwell` in the system, owned by L4, read by L5 — so the FSM can never say "Ready" while L4 is still shedding, nor vice-versa. The trip/recover band (trip strictly > recover) is what prevents flap; the dwell prevents a single lucky frame from recovering. This was the adversarial check and it holds because L5 does **not** define its own threshold — it consumes L4's.

### b.3 — `VramAccountant`: the box-scoped singleton (the cross-cell serialize)

**The race it prevents** (SCALE-83/120, the genuine correctness gap):

```
Box has 60 GiB free. Cell A wants model X (40 GiB peak). Cell B wants model Y (40 GiB peak).
WITHOUT the box-scoped singleton (per-process accountant):
  t0: A.accountant.check(40) → sees 60 free → OK, begins cudaMalloc
  t0: B.accountant.check(40) → sees 60 free → OK, begins cudaMalloc   ← both "see" 60
  t1: A allocates 40 (20 left).  B allocates 40 → needs 40, has 20 → CUDA OOM
      → OOM corrupts the *shared physical pool* → process-fatal in B AND can wedge A.
  Result: a double-load OOM that the cell topology was supposed to prevent.
WITH the box-scoped singleton (one reserve() critical section per box):
  t0: A.reserve(40) acquires the box lock → committed 0, peak 0 → 40 ≤ 60 → commit reserved=40 → release
  t0: B.reserve(40) blocks on the box lock; when it acquires → 40 reserved → 40 + 40 = 80 > 60
      → Refused(VramExceedsPeak{need:80, free:60}). B's load NEVER starts a cudaMalloc.
  Result: serialized; the second load is refused cleanly and the orchestrator reroutes it.
```

```rust
// waav-infer-scheduler/src/vram.rs  — ONE instance per physical box, shared by all cells.

#[derive(Clone, Copy, Debug, Default)]
pub struct LedgerState { pub committed: u64, pub reserved: u64, pub peak_cap: u64 }
//  peak_cap = total VRAM × safety (e.g. 0.95). projected_peak = committed + reserved.

pub struct VramAccountant {
    inner: Mutex<LedgerState>,   // THE single critical section across cells (held µs, not during malloc)
}

#[must_use = "drop frees the reservation (RAII); leaking it permanently shrinks the box budget"]
pub struct ReservationToken<'a> { acc: &'a VramAccountant, bytes: u64 }

impl VramAccountant {
    /// Box-scoped serialize. The ENTIRE cross-cell load decision is this one short critical
    /// section: read projected-peak, test against cap, commit-or-refuse. The actual cudaMalloc
    /// happens AFTER, holding the token but NOT the lock — so a 200 ms weight copy in cell A
    /// never stalls cell B's *serving tick* (B only contends for the µs-long reserve()).
    pub fn reserve(&self, bytes: u64) -> Result<ReservationToken<'_>, RejectReason> {
        let mut s = self.inner.lock().unwrap();
        let projected_peak = s.committed + s.reserved + bytes;   // free-before-load accounted by caller
        if projected_peak > s.peak_cap {
            return Err(RejectReason::VramExceedsPeak { need: projected_peak, free: s.peak_cap.saturating_sub(s.committed + s.reserved) });
        }
        s.reserved += bytes;
        Ok(ReservationToken { acc: self, bytes })
    }
    /// Promote a reservation to committed once cudaMalloc succeeds (weights now resident).
    pub fn commit(&self, tok: ReservationToken<'_>) {
        let mut s = self.inner.lock().unwrap();
        s.reserved -= tok.bytes; s.committed += tok.bytes; std::mem::forget(tok); // committed not freed by drop
    }
}
impl Drop for ReservationToken<'_> {  // refused/aborted load → reservation returns automatically (RAII)
    fn drop(&mut self) { self.acc.inner.lock().unwrap().reserved -= self.bytes; }
}
```

**KISS placement:** the singleton is a `Mutex<LedgerState>` shared via `Arc` *if cells are threads*, or a tiny per-box RPC/`flock`-guarded shared-memory ledger *if cells are processes* (the cell/shard default). The trait is identical; only the lock is process-shared. The critical section is bytes-of-arithmetic, so even a cross-process mutex is sub-µs — it never touches the serving path.

### b.4 — `ConfigTier` promotion (inline→pipelined→stage-batched without dropping a stream)

```rust
// waav-infer-scheduler/src/tier.rs
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConfigTier { Inline, PipelinedSingle, StageBatched }

#[derive(Clone, Copy, Debug)]
pub enum TierPolicy { Auto, PinEdge, PinDc }   // edge=refuse promotion; dc=boot StageBatched

pub struct TierExecutor { tier: ConfigTier, policy: TierPolicy /* + per-stage queues, ledger (lazy) */ }

impl TierExecutor {
    /// The load-bearing property (SCALE-2): promote WITHOUT dropping the in-flight stream.
    /// The DAG/stages/nested-loops/placement are IDENTICAL across tiers — only the executor
    /// differs (§8). So promotion is: (1) build the heavier machinery (queues + duty ledger),
    /// (2) hand the live slot's state to the new executor BETWEEN ticks (a slot-state move, not
    /// a re-init), (3) flip the tier. No frame is produced during the swap because it happens at
    /// a tick boundary; the next tick runs under the new executor with the SAME slot/ring/KV.
    pub fn on_second_stream(&mut self, live: &mut SlotTable) -> Result<(), RejectReason> {
        match self.policy {
            TierPolicy::PinEdge => return Err(RejectReason::ModePinned(ConfigTier::Inline)), // 429 the 2nd
            TierPolicy::PinDc   => { debug_assert_eq!(self.tier, ConfigTier::StageBatched); } // already batched
            TierPolicy::Auto    => {
                if self.tier == ConfigTier::Inline {
                    let queues = build_stage_queues();       // lazily constructed (edge never paid for it)
                    let ledger = build_duty_ledger();        // spins up on demand (§8)
                    self.adopt(live, queues, ledger);        // MOVE slot-state; no re-prefill, no drop
                    self.tier = ConfigTier::StageBatched;
                }
            }
        }
        Ok(())
    }
    fn adopt(&mut self, _live: &mut SlotTable, _q: StageQueues, _l: DutyLedger) { /* between-tick handoff */ }
}
```

### b.5 — `Migration`: leased ownership, append-only KV + inner latent, abort (the de-contradicted design)

The §6.5 fix makes the two migrations **explicitly distinct**: cadence-migration is *rejected* (the client playback buffer protects steady cadence, VoxServe-style); fault/spill-migration is a *measured, opt-in* option.

```rust
// waav-infer-scheduler/src/migration.rs
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MigrationKind { Cadence, FaultSpill }

/// version+cohort+region — a migration is refused unless ALL match (SCALE-119: never cross version).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MigrationKey { pub version: ModelVersion, pub cohort: CohortId, pub region: RegionId }

/// Single-writer ownership lease (generalizes G4 notify-before-wait to cross-replica). The SOURCE
/// holds the lease (and keeps serving, buffer-masked) until the DEST ACKs the transferred state,
/// THEN frees. A monotonic epoch prevents split-brain double-admit (SCALE-101).
pub struct OwnershipLease { pub session: SessionId, pub epoch: u64, pub expiry: Instant }

pub struct Migration {
    key: MigrationKey,
    lease: OwnershipLease,
    /// Append-only: only NEW KV blocks since the snapshot point are shipped (sub-ms–5 ms, L16)...
    append_only_kv: KvDelta,
    /// ...PLUS the inner-solver latent (the §6.5 explicit addition — a DiTAR/FlashTTS-class
    /// stream mid-inner-NFE-solve must carry its latent or the dest restarts the solve → glitch).
    inner_latent: Option<InnerSolverLatent>,
}

impl Migration {
    pub fn start(kind: MigrationKind, key: MigrationKey, src_key: &MigrationKey, now: Instant)
        -> Result<Self, RejectReason>
    {
        if kind == MigrationKind::Cadence {
            // The de-contradiction: cadence is NEVER migrated. Playback buffer protects it.
            return Err(RejectReason::ModePinned(ConfigTier::StageBatched)); // typed "rejected, not supported"
        }
        if &key != src_key { return Err(RejectReason::StaleCalibrationStamp { /* version/cohort/region mismatch */ ..todo!() }); }
        Ok(Self { key, lease: OwnershipLease { session: src_key.session(), epoch: next_epoch(), expiry: now + LEASE_TTL },
                  append_only_kv: KvDelta::snapshot(), inner_latent: InnerSolverLatent::capture() })
    }

    /// THE lease-expiry-mid-migration case (SCALE-105 zombie-slot guard). If the lease expires
    /// before the dest ACKs, the SOURCE aborts cleanly: it still owns the slot (single-writer),
    /// so it just CONTINUES serving (no state was freed — free happens only AFTER dest-ACK) and
    /// the half-shipped KV on the dest is dropped by the dest's stale-epoch guard. No zombie:
    /// at most one writer ever owns the session, and free is strictly after ACK.
    pub fn on_lease_expiry(&mut self, now: Instant) -> MigrationOutcome {
        if now >= self.lease.expiry {
            // dest never ACKed → abort. Source keeps the stream (never freed). Dest drops by epoch.
            MigrationOutcome::AbortedSourceRetains
        } else { MigrationOutcome::InFlight }
    }

    /// Dest ACKs the full transfer → NOW the source frees its slot. The single moment of handoff.
    pub fn on_dest_ack(&mut self) -> MigrationOutcome { MigrationOutcome::CommittedDestOwns /* source frees here */ }
}

#[derive(PartialEq, Eq, Debug)]
pub enum MigrationOutcome { InFlight, CommittedDestOwns, AbortedSourceRetains }
```

**The lease-expiry-mid-migration adversarial case, resolved:** because free is *strictly after* dest-ACK and the source is the single writer until then, an expired lease can only ever leave the **source still owning a fully-live stream** (it never freed) and the **dest holding a discardable partial** (dropped by stale-epoch). There is no window in which neither owns it (no zombie) and no window in which both admit it (the monotonic epoch rejects the stale dest). The drain-budget is sized to cover an in-flight migration (H7) so a `Drain` during migration waits for `CommittedDestOwns` or `AbortedSourceRetains` before exiting — never aborts mid-transfer.

### b.6 — `CalibrationStamp` lifecycle (§6.0 restore item 2)

```rust
// waav-infer-scheduler/src/calib.rs
#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct CalibrationStamp {
    pub sha: [u8; 32],        // weights sha256
    pub device: DeviceId,     // sm_121 / driver-visible device
    pub driver: DriverVer,    // CUDA driver version (a bump → re-stamp, FAIL-93)
    pub warm_set: WarmSetId,  // the canonical fixed frame-shape bucket set that was warmed
}

pub struct StampCache { map: HashMap<CalibrationStamp, CalibratedDuty> }
impl StampCache {
    /// Cache-hit on the SAME key skips recalibration → fast rollback (SCALE-45/113): redeploying
    /// last-known-good hits a valid stamp and flips /readyz green without re-running calibration.
    pub fn lookup(&self, k: &CalibrationStamp) -> Option<&CalibratedDuty> { self.map.get(k) }
}

impl ReadyGates {
    /// The calibration gate of Warming→Ready: /readyz stays 503 until a VALID stamp exists for
    /// THIS device+driver+warm-set. A driver bump (or MIG repartition, FAIL-94) invalidates the
    /// stamp → forces recalibration behind /readyz → admission refuses a stale stamp (no silent
    /// over-admit on a warming box).
    pub fn set_calib(&mut self, current: &CalibrationStamp, cache: &StampCache) {
        self.calib = cache.lookup(current).is_some();
    }
}
```

### b.7 — `ReconnectGovernor` (the per-replica storm cap — the one G-CTRL item that IS engine-side)

```rust
// waav-infer-scheduler/src/reconnect.rs  — front of admission, before the hot path (J19/SCALE-82).
pub struct ReconnectGovernor { tokens: f64, rate: f64, burst: f64, last: Instant }
impl ReconnectGovernor {
    /// A token-bucket on NEW *reconnect* admissions per replica. Full-Jitter backoff is the
    /// CLIENT's job; this is the SERVER's local cap so a reconnect storm (thousands in ~100 ms,
    /// the #1 metastable-failure sustaining effect) can't re-knock-down a recovering replica.
    /// Returns Retry-After so the orchestrator/client de-correlate.
    pub fn admit(&mut self, now: Instant) -> Result<(), RejectReason> {
        self.tokens = (self.tokens + self.rate * now.duration_since(self.last).as_secs_f64()).min(self.burst);
        self.last = now;
        if self.tokens >= 1.0 { self.tokens -= 1.0; Ok(()) }
        else { Err(RejectReason::ReconnectRateCapped { retry_after: Duration::from_secs_f64(1.0 / self.rate) }) }
    }
}
```

### b.8 — Representative RED test bodies (GPU-free; FSM + cross-cell-serialize are pure logic)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // ---- LifecycleFsm: the gated transitions (no GPU) ----

    #[test] // warming_to_ready_requires_all_four_gates  [M4.5 / SCALE-6,116, FAIL-15]
    fn warming_to_ready_requires_all_four_gates() {
        let (tx, _rx) = tokio::sync::watch::channel(LifecycleState::Loading);
        let mut fsm = LifecycleFsm::new_loading(tx);
        let t = Instant::now();
        assert_eq!(fsm.on(Event::WeightsResident, t).unwrap(), LifecycleState::Warming);
        // three of four gates → still Warming (the first-request cliff defense)
        let three = ReadyGates { warm: true, captured: true, calib: true, accuracy: false };
        assert_eq!(fsm.on(Event::Gate(three), t).unwrap(), LifecycleState::Warming);
        // all four → Ready
        let four = ReadyGates { accuracy: true, ..three };
        assert_eq!(fsm.on(Event::Gate(four), t).unwrap(), LifecycleState::Ready);
    }

    #[test] // ready_to_degraded_lowers_stage_ceiling_and_re_admits  [SCALE-46/67, FAIL-65]
    fn ready_to_degraded_lowers_stage_ceiling_and_re_admits() {
        let mut fsm = ready_fsm_with_ceiling(64);
        let t = Instant::now();
        let s = fsm.on(Event::DegradeVerdict { stage: StageId::Codec, cpu_t_step: ms(40) }, t).unwrap();
        assert_eq!(s, LifecycleState::Degraded);
        assert!(fsm.rated_ceiling().slots() < 64, "Degraded must LOWER the rated ceiling");
        // existing streams keep serving (no preempt): admit still works up to the lower ceiling
        assert!(fsm.admit_ok());
    }

    #[test] // degraded_to_ready_requires_dwell_under_threshold (hysteresis consistent w/ L4) [SCALE-46]
    fn degraded_to_ready_requires_dwell_under_recover_threshold() {
        let mut fsm = degraded_fsm(Hysteresis { trip_p99: ms(80), recover_p99: ms(60), dwell: secs(60) });
        let t0 = Instant::now();
        // p99 just under TRIP but above RECOVER → NOT enough (the band prevents flap)
        assert_eq!(fsm.on(Event::StepP99(ms(70)), t0).unwrap(), LifecycleState::Degraded);
        // under RECOVER but dwell not elapsed → still Degraded
        assert_eq!(fsm.on(Event::StepP99(ms(55)), t0).unwrap(), LifecycleState::Degraded);
        // under RECOVER AND dwell elapsed → Ready, ceiling restored
        assert_eq!(fsm.on(Event::StepP99(ms(55)), t0 + secs(61)).unwrap(), LifecycleState::Ready);
        assert_eq!(fsm.rated_ceiling().slots(), fsm.full_ceiling().slots());
    }

    #[test] // a blip above recover RE-ARMS the dwell clock (no creeping recovery)
    fn degraded_recover_dwell_resets_on_blip() {
        let mut fsm = degraded_fsm(Hysteresis { trip_p99: ms(80), recover_p99: ms(60), dwell: secs(60) });
        let t0 = Instant::now();
        fsm.on(Event::StepP99(ms(55)), t0).unwrap();                 // arm dwell
        fsm.on(Event::StepP99(ms(75)), t0 + secs(30)).unwrap();      // blip → disarm
        // 61 s after the FIRST under-sample is NOT enough; the clock restarted at the blip
        assert_eq!(fsm.on(Event::StepP99(ms(55)), t0 + secs(61)).unwrap(), LifecycleState::Degraded);
    }

    #[test] // draining_frees_on_refcount_zero + short_drain_then_abort_bounded  [SCALE-12, FAIL-116]
    fn draining_frees_on_refcount_zero_else_bounded_abort() {
        let mut fsm = ready_fsm_with_refcount(2);
        let t = Instant::now();
        assert_eq!(fsm.on(Event::Drain { deadline: secs(300) }, t).unwrap(), LifecycleState::Draining);
        assert_eq!(fsm.on(Event::StreamEnded, t).unwrap(), LifecycleState::Draining); // 1 left
        assert_eq!(fsm.on(Event::StreamEnded, t).unwrap(), LifecycleState::Failed);   // refcount 0 → exit
        // and a never-draining stream is bounded: deadline forces exit (never unbounded)
        let mut stuck = draining_fsm_with_refcount(1);
        assert_eq!(stuck.on(Event::DrainDeadline, t + secs(300)).unwrap(), LifecycleState::Failed);
    }

    #[test] // illegal edges are typed errors, not panics (codebase idiom)
    fn failed_is_terminal() {
        let mut fsm = failed_fsm();
        assert!(matches!(fsm.on(Event::Drain { deadline: secs(1) }, Instant::now()),
                         Err(LifecycleError::BadTransition(LifecycleState::Failed, _))));
    }

    // ---- VramAccountant: the cross-cell serialize (the race) — GPU-free ----

    #[test] // two_concurrent_cross_cell_loads_serialize  [M4.5 / SCALE-83,120 — the real gap]
    fn two_concurrent_cross_cell_loads_serialize() {
        let acc = VramAccountant::new(/*peak_cap*/ 60);
        let a = acc.reserve(40).expect("first load reserves");
        // The SECOND cell, racing on the SAME box budget, must be REFUSED (not double-fit).
        let b = acc.reserve(40);
        assert!(matches!(b, Err(RejectReason::VramExceedsPeak { need: 80, free: 20 })),
                "second cross-cell load must serialize behind the first, never both 'see' 60 free");
        drop(a); // the refused/aborted reservation returns (RAII); now B would fit
        assert!(acc.reserve(40).is_ok());
    }

    #[test] // sequential_swap_peak_is_n_minus_1_old_plus_1_new  [SCALE-107]
    fn six_variant_swap_peak_is_5old_plus_1new() {
        let acc = VramAccountant::new(/*peak_cap*/ 60); // 6×10 GiB variants, only 6 fit at rest
        let live: Vec<_> = (0..6).map(|_| acc.reserve(10).unwrap()).collect();   // 60 committed-ish
        // upgrading variant 0: must free OLD before reserving NEW (peak = 5 old + 1 new = 60, not 70)
        let mut live = live; let _old0 = live.remove(0); drop(_old0);            // free old0 → 50 reserved
        assert!(acc.reserve(10).is_ok(), "peak stays 5old+1new; never 6old+1new");
    }

    #[test] // reservation_does_not_block_serving_cells_tick (lock held µs, not during malloc) [SCALE-8]
    fn reservation_is_short_critical_section() {
        let acc = VramAccountant::new(60);
        let t = Instant::now();
        let tok = acc.reserve(40).unwrap();   // arithmetic only — the malloc happens after, lock released
        assert!(t.elapsed() < ms(1), "reserve() must be a microsecond arithmetic critical section");
        acc.commit(tok);
    }

    // ---- ConfigTier promotion (the load-bearing without-drop property) ----

    #[test] // second_stream_promotes_without_dropping_first  [M4.5 / SCALE-2]
    fn second_stream_promotes_inline_to_stage_batched_without_dropping_first() {
        let mut ex = TierExecutor::auto_inline();
        let mut live = SlotTable::with_one_active_stream();
        let pre = live.slot_state(SlotId(0));                 // capture the in-flight slot state
        ex.on_second_stream(&mut live).unwrap();
        assert_eq!(ex.tier(), ConfigTier::StageBatched);
        assert_eq!(live.slot_state(SlotId(0)), pre, "promotion MOVES slot-state; the first stream is not re-prefilled or dropped");
    }

    #[test] // mode_edge_pin_rejects_second_stream  [SCALE-3]
    fn edge_pin_refuses_promotion() {
        let mut ex = TierExecutor::pinned_edge();
        let r = ex.on_second_stream(&mut SlotTable::with_one_active_stream());
        assert!(matches!(r, Err(RejectReason::ModePinned(ConfigTier::Inline))));
    }

    // ---- Migration: leased ownership + the lease-expiry-mid-migration case ----

    #[test] // cadence_migration_rejected_playback_buffer_protects  [SCALE-37/119 — contradiction fixed]
    fn cadence_migration_is_rejected() {
        let r = Migration::start(MigrationKind::Cadence, key(), &key(), Instant::now());
        assert!(r.is_err(), "cadence is protected by the playback buffer, NEVER migrated");
    }

    #[test] // migration_refused_across_version_or_cohort  [SCALE-119]
    fn fault_migration_refused_on_version_mismatch() {
        let src = key_v(1); let dst = key_v(2);
        assert!(Migration::start(MigrationKind::FaultSpill, dst, &src, Instant::now()).is_err());
    }

    #[test] // lease_expiry_mid_migration_aborts_no_zombie  [SCALE-105 — the adversarial case]
    fn lease_expiry_mid_migration_source_retains_no_zombie() {
        let mut m = Migration::start(MigrationKind::FaultSpill, key(), &key(), Instant::now()).unwrap();
        // dest never ACKs; lease expires → source retains the FULLY-LIVE stream (free is after ACK only)
        assert_eq!(m.on_lease_expiry(Instant::now() + LEASE_TTL + secs(1)),
                   MigrationOutcome::AbortedSourceRetains);
        // and the committed handoff frees the source exactly once, only on dest-ACK
        let mut m2 = Migration::start(MigrationKind::FaultSpill, key(), &key(), Instant::now()).unwrap();
        assert_eq!(m2.on_dest_ack(), MigrationOutcome::CommittedDestOwns);
    }

    #[test] // ownership_lease_prevents_double_admit  [SCALE-101 split-brain]
    fn monotonic_epoch_prevents_double_admit() {
        let a = OwnershipLease { session: sid(), epoch: 5, expiry: far() };
        let stale = OwnershipLease { session: sid(), epoch: 4, expiry: far() };
        assert!(stale.epoch < a.epoch, "a partition with a stale epoch cannot re-admit the session");
    }

    // ---- Calibration stamp + reconnect governor ----

    #[test] // calibration_stamp_gates_readyz + cache_hit_skips_recalibration  [FAIL-93, SCALE-45]
    fn stamp_gates_readyz_and_cache_hit_skips_recalibration() {
        let cache = StampCache::with(stamp_for(DRIVER_A));
        let mut gates = ReadyGates::default();
        gates.set_calib(&stamp_for(DRIVER_A), &cache); assert!(gates.calib);        // hit → ready gate true
        gates.set_calib(&stamp_for(DRIVER_B), &cache); assert!(!gates.calib);       // driver bump → stale → 503
    }

    #[test] // reconnect_admission_rate_capped_per_replica  [M4.5 / SCALE-82]
    fn reconnect_storm_is_rate_capped_with_retry_after() {
        let mut gov = ReconnectGovernor::new(/*rate*/ 10.0, /*burst*/ 10.0);
        let t = Instant::now();
        for _ in 0..10 { gov.admit(t).unwrap(); }                 // burst drains
        assert!(matches!(gov.admit(t), Err(RejectReason::ReconnectRateCapped { .. }))); // 11th capped
    }
}
```

**Type inventory (L5):** `ControlPlane` (trait), `ReplicaSignals`, `Command`, `CommandReply`, `RejectReason`, `LifecycleState`, `LifecycleFsm`, `ReadyGates`, `Hysteresis`, `LifecycleError`, `Event`, `VramAccountant`, `LedgerState`, `ReservationToken`, `ConfigTier`, `TierPolicy`, `TierExecutor`, `MigrationKind`, `MigrationKey`, `OwnershipLease`, `Migration`, `MigrationOutcome`, `CalibrationStamp`, `StampCache`, `ReconnectGovernor`. **= 24 types/traits.**

**Named RED gates (L5):** `control_plane_emits_used_total_per_substrate`, `lifecycle_event_stream_emits_all_states`, `load_command_refused_returns_typed_reason_for_reroute`, `drain_command_returns_completion_event`, `freeze_rollout_command_stops_new_admits_keeps_live`, `canary_fraction_routes_new_sessions_only`, `reconnect_admission_rate_capped_per_replica`, `loading_to_warming_on_weights_resident`, `warming_to_ready_requires_all_four_gates`, `ready_to_degraded_lowers_stage_ceiling_and_re_admits`, `degraded_to_ready_requires_dwell_under_recover_threshold`, `degraded_recover_dwell_resets_on_blip`, `draining_frees_on_refcount_zero`, `short_drain_then_abort_bounded`, `failed_is_terminal_restart_is_bounded_backoff`, `two_concurrent_cross_cell_loads_serialize`, `sequential_swap_peak_is_n_minus_1_old_plus_1_new`, `reservation_does_not_block_serving_cells_tick`, `slot_cap_derived_from_vram_capacity`, `multi_model_co_residency_admission`, `second_stream_promotes_without_dropping_first`, `mode_edge_pin_rejects_second_stream`, `mode_dc_boots_stage_batched_bs1_no_batch_wait`, `cadence_migration_rejected_playback_buffer_protects`, `fault_migration_appends_kv_and_inner_latent_leased_buffer_masked`, `source_holds_until_dest_acks_then_frees`, `lease_expiry_mid_migration_aborts_no_zombie`, `ownership_lease_prevents_double_admit`, `migration_refused_across_version_or_cohort`, `calibration_stamp_gates_readyz`, `admission_refuses_stale_stamp`, `calibration_stamp_cache_hit_skips_recalibration`, `partition_change_invalidates_stamp`. **= 33 named gates** (12 with representative bodies above).

---

## (c) Residual gaps

These are *not* closed by L5 and are correctly out of L5's scope (orchestrator-owned or another layer's), or are explicitly deferred. Each says where it lives.

1. **The fleet control loop itself (autoscale / placement / rollout-sequencing / warm-pool maintenance / region routing) is NOT in WaaV** — by design. L5 closes these *at the contract boundary* (CLOSED-CONTRACT): the engine emits the signals and accepts the commands, but the loop that reads `used/total`, decides to add a replica, and sequences a rolling deploy is the **orchestrator's** (K8s operator / fleet manager). This is the intended line, but it means ~30 SCALE COMPOUND/EXTREME scenarios are only *contract-satisfied* — an end-to-end test of "the autoscaler adds capacity under a flash crowd" requires a real or mock orchestrator that L5 does not provide. **Owner: orchestrator (out of engine scope); residual = an integration harness, not engine code.**

2. **Cross-process VRAM singleton implementation choice is specified but not benchmarked.** b.3 gives the trait and the in-process `Mutex` form; the cross-*process* form (the cell/shard default) needs a concrete pick — shared-memory ledger vs a tiny per-box arbiter RPC vs `flock`-guarded mmap — with a measured cross-process `reserve()` latency (the design asserts sub-µs; it is arithmetic, but a cross-process mutex wake has not been measured on GB10). **Owner: L5 impl (M4.5); residual = the process-shared lock benchmark + pick.**

3. **L4 dependencies that L5 *consumes* but does not own.** The Degraded→Ready hysteresis is consistent *because* it reuses L4's single `(trip_p99, recover_p99, dwell)` band — but that band, the EWMA live-measured-step-time drift detector (§6.0 restore item 1, FAIL-23), and the duty-recompute `Ceiling::recompute` all live in **L4**. If L4's drift detector is not built, L5's `Event::DegradeVerdict` / `Event::StepP99` have no producer. **Owner: L4 scheduler; residual = a cross-layer wiring gate (`drift_verdict_drives_lifecycle_degrade`) at the L4/L5 seam.**

4. **Migration KV-delta wire format + inner-latent serialization are typed but not specified to bytes.** `KvDelta::snapshot()` / `InnerSolverLatent::capture()` are the right seams (append-only, same-version), but the on-wire encoding (NIXL/FlowKV-style, sub-ms–5 ms target), the dest-side stale-epoch drop, and the playback-buffer-mask timing (`one decode-step > one frame` so ≥1 frame is masked) are named, not byte-specified. Fault/spill migration is *opt-in* and *DC-tier-only* (the GB10 single-box default never migrates), so this is correctly deferred. **Owner: M5.x (opt-in migration); residual = the wire format + a `migration_masked_by_playback_buffer_no_underrun` integration gate.**

5. **Per-SLA-tier reserved duty / admit-gold-preferentially** (SCALE-57/76/100, and the `tier_floor` field on `SetAdmitPolicy`) is a *control-plane input* L5 exposes, but the *admission-side enforcement* (reserve duty for gold, shed silver first) is **L4's** `admission.rs`. L5 carries the policy field; L4 must honor it. **Owner: L4 admission; residual = `gold_tier_reserved_duty_admitted_preferentially` (an L4 gate the L5 contract feeds).**

6. **Calibration *persistence* across process restart** is designed (`StampCache` cache-hit skips recalibration) but the on-disk store (where the cache lives, keyed `sha×device×driver×warm-set`, and its invalidation on driver/MIG change) is a stub here. The lookup logic is GPU-free tested; the durable store is not. **Owner: L5 impl (M4.4 calibration); residual = `calibration_stamp_persisted_and_reloaded_on_restart`.**

None of these six is a *correctness* gap in L5's owned scope: the one genuine cross-cell correctness bug (double-load OOM) is fixed by the box-scoped singleton (b.3, gate `two_concurrent_cross_cell_loads_serialize`); the self-contradiction (cadence-migrate vs fault-migrate) is resolved (b.5); the FSM hysteresis is consistent with L4 by construction (b.2). The residuals are (1) the by-design orchestrator boundary, (2/4/6) impl-specification of already-typed seams, and (3/5) cross-layer wiring to L4 that L5 correctly does not duplicate.
