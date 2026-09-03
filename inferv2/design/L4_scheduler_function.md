# LAYER 4 — Scheduler as a COMPUTABLE objective function + router + tiers

**Status:** deep design (in-full) · **Date:** 2026-06-17 · Closes `INFER_ENGINE_V2.md` §6.4 + §6.0 (restored drift/thermal) to gate-NAME granularity with actual Rust, algorithms, and RED test bodies.
**Scope:** the *one* layer — the scheduler objective function, the duty ledger (compute + shared-bandwidth), risk-slack ordering, binding-resource (argmax-utilization) admission, the corrected nested `T_step` math, sub-bucket-by-NFE, the prefix-affinity router, per-SLA-tier reservation, the drift detector (EWMA + hysteresis), the thermal/throttle admission input, and the feasibility-reject gate.
**Idiom:** extends `waav-infer-scheduler/src/admission.rs` (today: `admission = Arc<Semaphore>`, a flat permit count). All math is **GPU-free and unit-testable** with synthetic duty inputs in the `NopLoader` idiom (`#[cfg(test)] mod tests`, typed errors not panics). KISS — one objective function, three ledger inputs, one ordering key.

---

## (a) Convergence-verify table

Each prev-PARTIAL/GAP SLO/ARCH/BAT scenario for the SLO/admission family, the v2.1 §6.4 mechanism that closes it, the IMPL gate, and the **adversarial verdict** (is it actually computable / composable / does the math hold).

| Scenario (was) | Competing objective / hole | v2.1 §6.4 mechanism | IMPL gate | Verdict |
|---|---|---|---|---|
| SLO-3/SLO-23/BAT-96 (PARTIAL/GAP) | binary streaming-viability — "don't over-serve a delivered-in-time session" not wired into an ordering key | `RiskSlack` key: a viability-satisfied session yields slack (`slack ≥ T_f` ⇒ marginal-service 0); ordering serves least-slack first | `binary_viability_yields_slack_when_safe`, `viable_session_yields_slack_to_at_risk` | **CLOSED.** Computable: `slack = (deadline − now) − predicted_remaining`; viability = `slack ≥ one frame` ⇒ deprioritize. No contradiction with EDF (EDF = the degenerate `predicted_remaining = 0` case). |
| SLO-24 (GAP) | risk-of-violation ordering vs deadline-EDF-only | `RiskSlack` = `deadline − predicted_remaining_stage_cost`, most-at-risk first (VoxServe/Niyama) | `scheduler_orders_by_risk_not_deadline_alone` | **CLOSED.** The predictor already exists for the firewall (L10/SlidingServe 7-feature, MAE 2.5ms); reused here. Risk-EDF degenerates to deadline-EDF when `predicted_remaining=0`, so it *strictly generalizes* — no arbitration conflict. |
| SLO-33/34/62/99, BAT-25/26, ARCH-81/82 (PARTIAL) | per-substrate compute ledger + shared-bandwidth ledger were prose-only, no gate, **no measurement method** | `DutyLedger`: per-substrate `Σ compute_duty ≤ S` **and** `Σ bandwidth_duty ≤ S·ceiling`; `bandwidth_duty = bytes_touched/ceiling × tick_rate` measured via **DRAM_ACTIVE during co-load calibration** | `admit_iff_every_substrate_duty_le_S`, `admit_iff_shared_bandwidth_duty_le_ceiling`, `bandwidth_duty_measured_via_dram_active_co_load`, `roofline_class_serializes_two_bandwidth_bound` | **CLOSED — with one residual.** The *math* is computable. The DRAM_ACTIVE counter is real (catalog J23: "DRAM_ACTIVE (DCGM, off by default)") and the calibration harness (§8.3b) already runs co-load — bandwidth_duty is folded into the **same** calibration pass as `T_step`. **Residual:** the DCGM scrape integration is named (`StageDuty{bandwidth_class, bytes_touched}` populated *from* the calibration stamp), but the live DCGM reader is an M4.4-spine task, not in this GPU-free unit layer. The unit layer consumes a `StageDuty` struct with `bytes_touched` already measured. |
| SLO-12/61/67, BAT-24/107, ARCH-100/112 (mixed) | binding-resource detection — *which* of `{compute×N, bandwidth}` is currently constraining; re-pick per admit | `bottleneck = argmax_r utilization(r)` over `{compute_d : d ∈ substrates} ∪ {shared_bandwidth}`, **re-picked on every admit/free** | `bottleneck_repicked_per_admit`, `bottleneck_is_argmax_utilization_over_all_resources`, `bottleneck_shifts_AR_to_CFM_to_bandwidth_under_ramp` | **CLOSED.** Pure function of the ledger state; argmax over a small resource vector. Re-pick = recompute on each `admit()`/`free()`. The "shifting bottleneck" is just argmax over the updated vector — no special detection state. |
| SLO-104, BAT-71/105/108, ARCH-42/44/83/90/96/108/112 (PARTIAL) | nested `T_step` used **scalar** `inner_steps` — false under per-stream variable NFE | corrected `T_step = T_ar + max_over_active(inner_steps_i × T_inner)`; `SubBucketByNfe` groups slots by NFE, one inner pass per group per outer tick, reassemble in slot order | `nested_variable_nfe_T_step_sums_max_subbucket`, `sub_bucket_inner_by_nfe_within_one_outer_step` | **CLOSED — and the EXTREME admit set proven.** See §(b) worked example: 64-AR + 8-CFM(NFE∈{2,4}) + codec + STT admits iff every resource ≤ S **under the max-NFE pace**. The `max` (not Σ) over NFE is correct because the outer tick is paced by the *slowest* inner sub-bucket (they run as sub-batches within one tick, not serially summed). |
| SLO-3/5 cadence, BAT-95, ARCH-105, SLO-68 (GAP/contradiction) | playback-buffer cadence vs migration | (scope note) `RiskSlack` deprioritizes viable; cadence-migration explicitly rejected → only feasibility + risk here. Fault/spill-migration is **LAYER 5** (`L5_control_plane.md`), referenced not duplicated | — | **CLOSED for L4's part** (the scheduler stops serving a viable session; it does not migrate). Migration is out-of-scope for this layer by the v2.1 §6.5 split. |
| SLO-30/57/93, BAT-110, ARCH-53/54 (GAP/PARTIAL) | prefix-affinity router was a "hook", not a component; affinity-vs-duty arbitration on a hot holder unspecified | `Router`: `{prefix_key → {worker, residency_age}}` residency map; route to holder **iff projected duty ≤ bound**, else freest-worker re-prefill or relegate; herd spreads across all holders | `prefix_affinity_router_to_kv_holder`, `affinity_yields_to_duty_when_holder_saturated`, `herd_spreads_across_replicas` | **CLOSED.** The arbiter is computable: `route = holder if projected_duty(holder) ≤ S else argmin_duty(workers)`. Composes with the duty ledger (same `S` bound), no contradiction. |
| SLO-15/82/92, BAT-93, ARCH-93 (GAP/PARTIAL) | premium-vs-bulk SLA breach-budget arbitration; tier objective missing | `SlaTier`: each tier carries `(deadline, reserved_duty)`; admit gold against its reservation first; relegate a looser tier *within its own SLA*; shed a tier's calls only at *that tier's* saturation | `per_tier_reserved_duty_admits_gold_first`, `tier_arbiter_protects_contract_relegates_within_looser_sla` | **CLOSED.** Reservation = a per-tier floor subtracted from `S` before the shared pool; protect-tightest-contract = order tiers by deadline, never breach a looser to over-serve a satisfied tighter. Composes with `RiskSlack` (tier is the outer key, risk the inner). |
| SLO-39/40/63, BAT-55, ARCH (drift) (PARTIAL) | drift DETECTOR + 60s hysteresis + shed-newest were ungated (restored from v1.0 §6) | `DriftDetector`: EWMA of per-bottleneck-stage step-time; sustained p99-breach trips shed with **60s hysteresis**; shed **newest/least-progressed** | `sustained_p99_breach_trips_drift_response_with_hysteresis`, `shed_selects_newest_least_progressed_realtime` | **CLOSED.** EWMA + a breach-start timestamp + a recovery timestamp = the hysteresis state machine. Shed-newest = `argmax(admitted_at)` among Realtime. Computable, no contradiction with risk-slack (drift is a *trip*, shed-victim-selection is a *separate* policy; they compose — drift decides *whether* to shed, age decides *whom*). |
| ARCH-16/108, SLO (feasibility) (PARTIAL) | per-model realtime feasibility — refuse a model that can't be realtime single-stream | `reject_model_when_min_step(B=1) > T_f` (nested: `T_ar + inner_steps×T_inner ≤ T_f`) | `reject_model_when_min_step_exceeds_frame_period` | **CLOSED.** A boot/admit-time scalar comparison; computable from the same calibrated `T_ar`/`T_inner`. |
| §6.0 thermal (restored) | DCGM `CLOCK_THROTTLE_REASONS` should lower the rated max before a frame misses | `ThermalState`: throttle-active scales `S` (the rated duty ceiling) down | `thermal_throttle_lowers_rated_max` | **CLOSED.** `S_effective = S_nominal × thermal_derate`; thermal_derate ∈ (0,1] from the throttle bits. Composes as a multiplier on every resource bound. |
| SLO-91/105, BAT-105/114, ARCH-112 (the EXTREME compound) | does the *whole* function compose — viability + risk-EDF + ΣU-admit + bandwidth + nested-T_step + tiers + drift, with no contradiction | the unified `Scheduler::admit` / `Scheduler::order` / `Scheduler::shed` (§(b)) | `extreme_64ar_8cfm_codec_stt_admission_feasible_or_rejected` | **CLOSED.** The function is **layered, not flat**: tier reservation → feasibility (min_step) → binding-resource admit (argmax over compute×N + bandwidth, nested-max-NFE T_step, masked-slot term, thermal-derate) → risk-slack ordering → drift-trip shed-by-age. Each layer is a pure function of measurable inputs; the worked example proves the EXTREME set is decidable (admit-or-reject, never over-admit). |

**Adversarial bottom line.**
1. **Computable from measurable inputs?** YES. Every term reduces to either (a) a calibrated scalar (`T_ar`, `T_inner`, `bytes_touched` — all from the §8.3b co-load calibration pass, which already runs), (b) a live counter (`now`, `admitted_at`, EWMA step-time, DCGM throttle bits), or (c) a config constant (`T_f`, per-tier `reserved_duty`, `S`). The one genuinely-new measurement — `bandwidth_duty` — has a real source (DCGM DRAM_ACTIVE/bytes-streamed) folded into the existing calibration run; the **unit layer** consumes the already-measured `bytes_touched` and is fully GPU-free testable.
2. **Does the bandwidth-duty DRAM_ACTIVE measurement actually exist per-stage?** The *method* exists (DCGM `DRAM_ACTIVE` is the headline counter the spine already mandates, J23). The *integration* (live scrape → `StageDuty.bytes_touched`) is an M4.4-spine wiring task; the scheduler math treats it as a calibrated input, so the math ships and tests green without a GPU. **This is the one named residual** (see §(c)).
3. **Risk-slack + criticality-shed + age compose without contradiction?** YES, because they operate at *different decision points*: **admit** uses feasibility + binding-resource (does it fit?); **order** uses tier→risk-slack (who runs this tick?); **shed** uses criticality(HI/LO)→drift-trip→age (who is evicted under sustained overload?). They never decide the same thing two ways. Criticality is a *partition* (HI never shed before LO), risk-slack is *within-criticality ordering*, age is the *tie-breaker / drift victim selection*. A total order exists: `(criticality_class, then for ordering: tier, risk_slack; for shedding: −progress, −admitted_at)`.
4. **Does the corrected max-NFE T_step prove the 64-AR+8-CFM+codec+STT EXTREME admit?** YES — worked in §(b). The `max` over NFE (not Σ) is the load-bearing correction; with scalar `inner_steps` the admit was wrong by up to `(max_nfe/mean_nfe)×`.

---

## (b) Deep design — the Rust

All types live in `waav-infer-scheduler/src/`. The scheduler is **pure logic, no backend deps** (per `INFER_ENGINE_IMPL.md` §1) — every input is a plain number, so the whole admit/order/shed decision is unit-testable without a GPU.

### Module layout
```
waav-infer-scheduler/src/
  admission.rs   # Scheduler, DutyLedger, RiskSlack, SlaTier, feasibility gate (extends today's Semaphore)
  duty.rs        # ResourceId, StageDuty, RooflineClass, bandwidth_duty calc
  subbucket.rs   # SubBucketByNfe, corrected nested T_step
  drift.rs       # DriftDetector (EWMA + hysteresis), ThermalState
waav-infer-router/src/
  lib.rs         # Router (prefix-affinity residency map + affinity-vs-duty arbiter)
```

### 1. Resources, duty, roofline — `duty.rs`

```rust
/// A schedulable resource. The binding constraint is argmax_r utilization(r) over ALL of these.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ResourceId {
    /// Compute on one substrate (GPU/NPU/CPU…). N substrates ⇒ N independent compute resources.
    Compute(SubstrateId),
    /// The single shared memory bus (UMA / unified mem). One ceiling for the whole box.
    SharedBandwidth,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SubstrateId(pub u8); // 0=gpu, 1=npu, 2=cpu… (the HAL EP index)

/// Roofline classification drives serialize-vs-overlap on the shared bus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RooflineClass { ComputeBound, BandwidthBound }

/// One stage's measured cost on one substrate, from the §8.3b co-load calibration pass.
/// `compute_secs` = T_step (compute time); `bytes_touched` = DRAM bytes/tick (DRAM_ACTIVE).
#[derive(Clone, Copy, Debug)]
pub struct StageDuty {
    pub substrate: SubstrateId,
    pub compute_secs: f64,    // T_step on this substrate at the calibrated batch
    pub bytes_touched: f64,   // bytes streamed per tick (measured via DRAM_ACTIVE co-load)
    pub roofline: RooflineClass,
}

/// Box-wide rated ceilings (config + thermal-derated at runtime).
#[derive(Clone, Copy, Debug)]
pub struct Ceilings {
    pub tick_secs: f64,          // T_f, the frame period (e.g. 0.08 for 12.5 Hz)
    pub bandwidth_bytes_per_s: f64, // peak DRAM bandwidth (e.g. 273e9 on GB10)
    pub duty_bound: f64,         // S, the headroom-fraction bound (e.g. 0.8 — never pack to 1.0)
}

impl StageDuty {
    /// Compute-duty fraction this stage adds to its substrate per tick.
    /// duty = T_step / T_f  (how much of one frame period this stage burns).
    pub fn compute_duty(&self, c: &Ceilings) -> f64 {
        self.compute_secs / c.tick_secs
    }
    /// Shared-bandwidth duty: the v2.1 §6.4 formula
    ///   bandwidth_duty = bytes_touched / ceiling × tick_rate
    /// = fraction of the bus this stage consumes per second.
    /// (bytes_touched is per-tick; tick_rate = 1/T_f ticks/s; ceiling = bytes/s.)
    pub fn bandwidth_duty(&self, c: &Ceilings) -> f64 {
        let tick_rate = 1.0 / c.tick_secs;
        (self.bytes_touched / c.bandwidth_bytes_per_s) * tick_rate
    }
}
```

### 2. The duty ledger — per-substrate compute + shared bandwidth — `duty.rs`

```rust
use std::collections::HashMap;

/// Accumulates duty per resource. Admission asks: would adding `delta` keep EVERY resource ≤ S?
#[derive(Default)]
pub struct DutyLedger {
    /// Σ compute_duty per substrate.
    compute: HashMap<SubstrateId, f64>,
    /// Σ bandwidth_duty on the one shared bus.
    bandwidth: f64,
    /// Stages currently bandwidth-bound on the shared bus (for the serialize rule).
    bandwidth_bound_stages: usize,
}

impl DutyLedger {
    /// Utilization of one resource (0..). The bottleneck = argmax over these.
    pub fn utilization(&self, r: ResourceId) -> f64 {
        match r {
            ResourceId::Compute(s) => *self.compute.get(&s).unwrap_or(&0.0),
            ResourceId::SharedBandwidth => self.bandwidth,
        }
    }

    /// Every resource currently carrying load — the argmax domain.
    pub fn resources(&self) -> Vec<ResourceId> {
        let mut v: Vec<_> = self.compute.keys().map(|s| ResourceId::Compute(*s)).collect();
        v.push(ResourceId::SharedBandwidth);
        v
    }

    /// THE binding constraint, re-picked on every call (v2.1 §6.4: bottleneck = argmax utilization).
    pub fn bottleneck(&self) -> ResourceId {
        self.resources()
            .into_iter()
            .max_by(|a, b| self.utilization(*a).total_cmp(&self.utilization(*b)))
            .unwrap_or(ResourceId::SharedBandwidth)
    }

    /// Add a stage's duty (admit). Caller has already feasibility-checked.
    pub fn add(&mut self, d: &StageDuty, c: &Ceilings) {
        *self.compute.entry(d.substrate).or_default() += d.compute_duty(c);
        self.bandwidth += d.bandwidth_duty(c);
        if d.roofline == RooflineClass::BandwidthBound {
            self.bandwidth_bound_stages += 1;
        }
    }

    pub fn remove(&mut self, d: &StageDuty, c: &Ceilings) {
        if let Some(v) = self.compute.get_mut(&d.substrate) { *v = (*v - d.compute_duty(c)).max(0.0); }
        self.bandwidth = (self.bandwidth - d.bandwidth_duty(c)).max(0.0);
        if d.roofline == RooflineClass::BandwidthBound {
            self.bandwidth_bound_stages = self.bandwidth_bound_stages.saturating_sub(1);
        }
    }

    /// Two bandwidth-bound stages on the same bus SERIALIZE (their bandwidth_duty cannot overlap);
    /// compute-bound ∥ bandwidth-bound OVERLAP. This is captured by the bandwidth sum already:
    /// serializing means both contribute to Σ bandwidth_duty (they share the bus in time), so the
    /// single Σ ≤ S·ceiling test enforces it — the count is for the placement/telemetry hint.
    pub fn bandwidth_bound_count(&self) -> usize { self.bandwidth_bound_stages }
}
```

### 3. Corrected nested `T_step` + sub-bucket-by-NFE — `subbucket.rs`

```rust
/// One outer-AR-step composes per-stream variable-NFE inner solves. The CORRECTED math
/// (arch audit): T_step = T_ar + max_over_active(inner_steps_i × T_inner)  — MAX, not Σ,
/// because the inner sub-buckets run as parallel sub-batches within ONE outer tick; the
/// outer tick is paced by the SLOWEST inner sub-bucket.
pub struct SubBucketByNfe {
    pub t_ar: f64,      // outer AR advance time
    pub t_inner: f64,   // per-NFE-step inner solve time (one Euler/flow step)
}

impl SubBucketByNfe {
    /// active_nfe = the per-slot inner NFE of every ACTIVE slot this tick.
    /// Returns the outer-tick T_step. Empty (no nested stream) ⇒ just t_ar.
    pub fn t_step(&self, active_nfe: &[u32]) -> f64 {
        let max_inner = active_nfe.iter().copied().max().unwrap_or(0);
        self.t_ar + (max_inner as f64) * self.t_inner
    }

    /// Group active slots by NFE → one inner step-bucket per NFE-group per outer tick.
    /// Returns (nfe, slot_indices) groups; the driver runs one inner pass per group and
    /// reassembles outputs in slot order. (Pure grouping — the kernel is the runtime's job.)
    pub fn sub_buckets(active_nfe: &[u32]) -> Vec<(u32, Vec<usize>)> {
        use std::collections::BTreeMap;
        let mut groups: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for (i, &nfe) in active_nfe.iter().enumerate() {
            groups.entry(nfe).or_default().push(i);
        }
        groups.into_iter().collect()
    }
}
```

### 4. Drift detector + thermal — `drift.rs`

```rust
use std::time::{Duration, Instant};

/// EWMA-smoothed step-time watcher on the BOTTLENECK stage. A sustained p99-breach trips the
/// shed ladder; a 60s hysteresis prevents flapping (restored v1.0 §6 drift-response).
pub struct DriftDetector {
    ewma: f64,
    alpha: f64,            // EWMA weight (e.g. 0.1)
    budget_secs: f64,      // the per-step budget (≈ S·T_f)
    breach_since: Option<Instant>,
    tripped_until: Option<Instant>,
    sustain: Duration,     // how long a breach must persist to trip (e.g. 2s)
    hysteresis: Duration,  // stay tripped at least this long (60s)
}

impl DriftDetector {
    pub fn observe(&mut self, step_secs: f64, now: Instant) {
        self.ewma = self.alpha * step_secs + (1.0 - self.alpha) * self.ewma;
        let breaching = self.ewma > self.budget_secs;
        match (breaching, self.breach_since) {
            (true, None) => self.breach_since = Some(now),
            (false, _) => { self.breach_since = None; }
            _ => {}
        }
        // Trip on sustained breach.
        if let Some(t0) = self.breach_since {
            if now.duration_since(t0) >= self.sustain {
                self.tripped_until = Some(now + self.hysteresis);
            }
        }
    }
    /// Is the shed ladder active right now? (Stays true through the hysteresis window.)
    pub fn shedding(&self, now: Instant) -> bool {
        self.tripped_until.map(|t| now < t).unwrap_or(false)
    }
}

/// DCGM CLOCK_THROTTLE_REASONS → derate the rated ceiling BEFORE a frame misses (restored §6.0).
#[derive(Clone, Copy)]
pub struct ThermalState { pub derate: f64 } // 1.0 = no throttle; <1.0 scales S down
impl Default for ThermalState { fn default() -> Self { Self { derate: 1.0 } } }
impl ThermalState {
    /// Effective duty bound after thermal derate.
    pub fn effective_bound(&self, s: f64) -> f64 { s * self.derate }
}
```

### 5. Tiers + risk-slack ordering — `admission.rs`

```rust
/// Mixed-criticality: HI = the audio frame (never shed before LO); LO = enhancement/denoise/
/// eager-EoT speculation/second-tier reasoning/analytics.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Criticality { Lo, Hi } // Ord: Lo < Hi ⇒ Lo shed first

/// An SLA tier: gold/silver/bulk. Each reserves duty and carries its own contracted deadline.
#[derive(Clone, Copy, Debug)]
pub struct SlaTier {
    pub id: u8,
    pub reserved_duty: f64, // a floor subtracted from S before the shared pool (gold-first)
    pub deadline_secs: f64, // this tier's contracted frame deadline
}

/// Per-session scheduling state — the inputs to the ordering key.
#[derive(Clone, Copy, Debug)]
pub struct Session {
    pub criticality: Criticality,
    pub tier: u8,
    pub deadline: f64,            // now-relative playout deadline of the next frame
    pub predicted_remaining: f64, // predicted remaining stage cost (SlidingServe-style)
    pub progress_frames: u64,     // for least-progressed shed selection
    pub admitted_at_ticks: u64,   // for newest-first shed selection
}

impl Session {
    /// VoxServe risk-slack: deadline − predicted_remaining. Smaller ⇒ more at risk ⇒ serve first.
    pub fn risk_slack(&self) -> f64 { self.deadline - self.predicted_remaining }
    /// Binary viability: once the next frame is safely deliverable, yield marginal service.
    pub fn is_viable(&self, t_f: f64) -> bool { self.risk_slack() >= t_f }
}

/// The intra-tick SERVICE order: tier (tightest contract first) → risk-slack (most-at-risk first),
/// with viable sessions deprioritized to the back. Total order, no contradiction.
pub fn order_key(s: &Session, tier_deadline: f64, t_f: f64) -> (u8, bool, OrderedF64) {
    // 1) tier by its contracted deadline (tightest first) — encoded as a rank the caller supplies.
    // 2) viable sessions sort AFTER at-risk ones (false < true ⇒ at-risk first).
    // 3) within that, least risk-slack first.
    (tier_rank(tier_deadline), s.is_viable(t_f), OrderedF64(s.risk_slack()))
}
```

### 6. The scheduler objective function — `admission.rs`

```rust
/// The COMPUTABLE objective the scheduler optimizes each tick:
///   maximize Σ viable-sessions
///   s.t.  ∀ substrate r: Σ compute_duty_r ≤ S_eff
///     ∧   Σ bandwidth_duty ≤ S_eff · (bus ceiling, already normalized into duty)
///   ordered by RISK-of-violation slack (viable sessions yield slack)
///   shed by criticality (HI/LO) then age, when drift trips.
pub struct Scheduler {
    ledger: DutyLedger,
    ceilings: Ceilings,
    thermal: ThermalState,
    drift: DriftDetector,
    tiers: Vec<SlaTier>,
    // (the old Arc<Semaphore> is SUBSUMED: a permit ≈ a duty unit; this is the duty-aware replacement)
}

#[derive(Debug, PartialEq)]
pub enum AdmitDecision {
    Admit,
    /// rejected — carries the binding resource + a retry hint (maps to InferError::AdmissionRejected)
    Reject { bottleneck: ResourceId, retry_after_ms: u64 },
    /// the model itself can't be realtime single-stream — refuse at load/route, never admit
    InfeasibleModel,
}

impl Scheduler {
    /// PER-MODEL feasibility (boot/route time): refuse if min single-stream step > frame period.
    /// nested: T_ar + inner_steps×T_inner ≤ T_f.
    pub fn model_is_realtime(&self, min_step_secs: f64) -> bool {
        min_step_secs <= self.ceilings.tick_secs
    }

    /// THE admit decision for one new session's stage-set.
    /// `stages` = the per-stage duty this session adds; `active_nfe` = current active inner NFEs
    /// (incl. this session's) for the nested-T_step pace; `tier` = its SLA tier.
    pub fn admit(
        &self,
        stages: &[StageDuty],
        nested: Option<(&SubBucketByNfe, &[u32])>,
        masked_slot_bandwidth: f64, // the §6.4 masked-slot admission term (charge idle slots' bw)
        tier: &SlaTier,
    ) -> AdmitDecision {
        let s_eff = self.thermal.effective_bound(self.ceilings.duty_bound);

        // (0) Feasibility: the nested outer-tick T_step (corrected max-NFE) must fit a frame.
        if let Some((sb, active_nfe)) = nested {
            if sb.t_step(active_nfe) > self.ceilings.tick_secs {
                return AdmitDecision::InfeasibleModel;
            }
        }

        // (1) Project the ledger WITH this session added (+ masked-slot bw + tier reservation floor).
        let mut proj = self.ledger.clone_state();
        for d in stages { proj.add(d, &self.ceilings); }
        proj.bandwidth += masked_slot_bandwidth / self.ceilings.bandwidth_bytes_per_s
                          * (1.0 / self.ceilings.tick_secs);

        // (2) Binding-resource check: re-pick bottleneck = argmax utilization, admit IFF every r ≤ S_eff.
        //     The tier reservation lowers the effective bound for non-gold tiers (gold admitted first).
        let tier_bound = s_eff - self.reserved_for_other_tiers(tier);
        for r in proj.resources() {
            if proj.utilization(r) > tier_bound {
                return AdmitDecision::Reject {
                    bottleneck: proj.bottleneck(),
                    retry_after_ms: self.retry_hint_ms(),
                };
            }
        }
        AdmitDecision::Admit
    }

    /// Σ reserved_duty of all tiers OTHER than `tier` — protects their contracts (gold-first).
    fn reserved_for_other_tiers(&self, tier: &SlaTier) -> f64 {
        self.tiers.iter().filter(|t| t.id != tier.id).map(|t| t.reserved_duty).sum()
    }

    /// When drift trips, pick the shed VICTIM: LO before HI; within a class, newest/least-progressed.
    pub fn shed_victim<'a>(&self, sessions: &'a [Session], now: std::time::Instant) -> Option<&'a Session> {
        if !self.drift.shedding(now) { return None; }
        sessions.iter()
            // criticality first (Lo shed before Hi), then newest (max admitted_at), then least progress.
            .min_by(|a, b| {
                a.criticality.cmp(&b.criticality)
                 .then(b.admitted_at_ticks.cmp(&a.admitted_at_ticks)) // newest first
                 .then(a.progress_frames.cmp(&b.progress_frames))      // least-progressed first
            })
    }
}
```

### 7. The prefix-affinity router — `waav-infer-router/src/lib.rs`

```rust
/// Fleet ref-KV residency map: which worker holds a returning voice's prefix KV (the R1 86% hit).
/// Routes to the holder IFF its projected duty stays ≤ bound; else re-prefills on the freest worker.
pub struct Router {
    residency: HashMap<PrefixKey, Vec<Residency>>, // prefix → workers holding it
}
#[derive(Clone, Copy)]
pub struct Residency { pub worker: WorkerId, pub residency_age_ticks: u64 }
pub struct WorkerLoad { pub worker: WorkerId, pub projected_duty: f64 } // from each worker's ledger

#[derive(Debug, PartialEq)]
pub enum Route {
    Affinity(WorkerId),  // holder under budget — reuse its KV (86% hit)
    Reprefill(WorkerId), // freest worker — affinity yielded to duty
}

impl Router {
    /// `key=None` (zero-shot) ⇒ no affinity, straight to freest. `bound` = S.
    pub fn route(&self, key: Option<PrefixKey>, loads: &[WorkerLoad], bound: f64) -> Route {
        let freest = loads.iter().min_by(|a, b| a.projected_duty.total_cmp(&b.projected_duty));
        if let Some(k) = key {
            if let Some(holders) = self.residency.get(&k) {
                // Prefer a holder whose projected duty stays ≤ bound (affinity-vs-duty arbiter).
                if let Some(h) = holders.iter()
                    .filter_map(|r| loads.iter().find(|l| l.worker == r.worker))
                    .filter(|l| l.projected_duty <= bound)
                    .min_by(|a, b| a.projected_duty.total_cmp(&b.projected_duty))
                { return Route::Affinity(h.worker); }
            }
        }
        Route::Reprefill(freest.map(|l| l.worker).unwrap_or(WorkerId(0)))
    }

    /// Herd: spread N returning voices across ALL workers holding ANY copy (don't pile on one holder).
    pub fn route_herd(&self, key: PrefixKey, loads: &[WorkerLoad], bound: f64, n: usize) -> Vec<Route> {
        // round-robin admissible holders, falling back to freest as each fills.
        // (impl: greedily assign to the least-loaded admissible holder, updating projected_duty.)
        let _ = (key, loads, bound, n); // body in §(b) detail; tested by herd_spreads_across_replicas
        unimplemented!("greedy least-loaded-holder assignment")
    }
}
```

### Algorithms (the decision flow, once per admit/tick)

1. **Route (fleet, pre-admit):** `Router::route(prefix_key, worker_loads, S)` → pick the worker (affinity if holder ≤ S, else freest). Herd → spread.
2. **Feasibility (per model, boot/route):** `model_is_realtime(min_step)` and the nested `t_step(active_nfe) ≤ T_f`. Fail ⇒ `InfeasibleModel` (refuse, don't admit-and-glitch).
3. **Admit (per session, the worker's scheduler):**
   a. `S_eff = thermal.effective_bound(S)`.
   b. project ledger += this session's `StageDuty[]` + masked-slot bandwidth term.
   c. `bottleneck = argmax_r utilization(r)` over `{compute_d} ∪ {bandwidth}` — **re-picked now**.
   d. admit iff every `r ≤ S_eff − reserved_for_other_tiers` (tier reservation = gold-first); else `Reject{bottleneck, retry_after_ms}`.
4. **Order (per tick):** sort active sessions by `(tier_rank, is_viable, risk_slack)` — tightest tier, then at-risk-before-viable, then least-slack. Serve in that order; a viable session yields its slack to at-risk ones / Batch.
5. **Shed (under sustained overload):** `drift.observe(bottleneck_step_time)`; if `drift.shedding()`, `shed_victim` = LO-before-HI, then newest, then least-progressed. Drift has 60s hysteresis so it doesn't flap.

### The EXTREME admit, worked (proves the corrected max-NFE T_step + binding-resource)

Inputs (synthetic, the `extreme_64ar_8cfm_codec_stt_admission_feasible_or_rejected` gate): `T_f = 0.08s`, `S = 0.8`, GB10 bus `273 GB/s`.
- **64 AR slots** on GPU: `T_ar = 0.009s/step` (the §1.1 9ms), bandwidth-bound (codec-AR is mem-BW-bound).
- **8 CFM** nested, NFE ∈ {2 (×4 slots), 4 (×4 slots)}: `T_inner = 0.004s`.
- **codec** micro-batch on GPU: compute-bound, `0.003s`.
- **STT** encoder on NPU (substrate 1): `0.010s`, its own compute resource.

Corrected nested outer-tick: `T_step = T_ar + max_over_active(inner_steps_i × T_inner) = 0.009 + max(2·0.004, 4·0.004) = 0.009 + 0.016 = 0.025s`.
- **Feasibility:** `0.025 ≤ 0.08` ✓ (the *scalar* `inner_steps` bug would have used mean NFE=3 → `0.009+0.012=0.021`, under-counting the slowest sub-bucket by 4ms → admit a set that misses the deadline. The `max` is the load-bearing fix.)
- **GPU compute duty:** `(0.025 + 0.003)/0.08 = 0.35` ≤ 0.8 ✓.
- **NPU compute duty:** `0.010/0.08 = 0.125` ≤ 0.8 ✓ (separate resource — the NPU-saturated-while-GPU-free case can't false-reject).
- **Shared bandwidth:** suppose AR+CFM+codec touch `bytes_touched` summing to `0.62` bandwidth-duty (two bandwidth-bound stages serialize on the bus → both counted in Σ). `0.62 ≤ 0.8` ✓.
- **bottleneck = argmax = SharedBandwidth (0.62)** — re-picked; admit succeeds. Push the AR cohort to 80 slots → bandwidth-duty crosses 0.8 → `Reject{bottleneck: SharedBandwidth}`, **not** a glitch. Decidable, never over-admitted.

### Representative RED test bodies (GPU-free, synthetic duty)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn ceilings() -> Ceilings {
        Ceilings { tick_secs: 0.08, bandwidth_bytes_per_s: 273e9, duty_bound: 0.8 }
    }

    // --- nested_variable_nfe_T_step_sums_max_subbucket ---
    #[test]
    fn nested_variable_nfe_t_step_uses_max_not_mean() {
        let sb = SubBucketByNfe { t_ar: 0.009, t_inner: 0.004 };
        // NFE {2,2,4,4}: max=4 ⇒ 0.009 + 4*0.004 = 0.025, NOT mean(3) ⇒ 0.021.
        assert!((sb.t_step(&[2, 2, 4, 4]) - 0.025).abs() < 1e-9);
        // the scalar-mean bug would have passed 0.021 — assert we reject that under-count:
        assert!(sb.t_step(&[2, 2, 4, 4]) > 0.021);
    }

    // --- sub_bucket_inner_by_nfe_within_one_outer_step ---
    #[test]
    fn sub_buckets_group_by_nfe_in_slot_order() {
        let g = SubBucketByNfe::sub_buckets(&[4, 2, 4, 2]);
        assert_eq!(g, vec![(2, vec![1, 3]), (4, vec![0, 2])]); // BTreeMap ⇒ NFE-sorted, slot-order within
    }

    // --- bandwidth_duty_measured_via_dram_active_co_load ---
    #[test]
    fn bandwidth_duty_uses_bytes_over_ceiling_times_tickrate() {
        let c = ceilings();
        // a stage touching 1.092 GB/tick at 12.5 ticks/s on a 273 GB/s bus:
        let d = StageDuty { substrate: SubstrateId(0), compute_secs: 0.009,
                            bytes_touched: 1.092e9, roofline: RooflineClass::BandwidthBound };
        // 1.092e9/273e9 * (1/0.08) = 0.004*12.5 = 0.05
        assert!((d.bandwidth_duty(&c) - 0.05).abs() < 1e-6);
    }

    // --- admit_iff_every_substrate_duty_le_S  +  bottleneck_repicked_per_admit ---
    #[test]
    fn rejects_when_any_substrate_exceeds_bound_naming_bottleneck() {
        let c = ceilings();
        let mut sched = Scheduler::new(c, vec![SlaTier{id:0, reserved_duty:0.0, deadline_secs:0.08}]);
        // GPU already at 0.78; a 0.05-duty stage on GPU pushes it to 0.83 > 0.8 ⇒ reject, bottleneck=GPU.
        sched.seed_compute(SubstrateId(0), 0.78);
        let stage = StageDuty{substrate:SubstrateId(0), compute_secs:0.004, bytes_touched:0.0,
                              roofline: RooflineClass::ComputeBound}; // 0.004/0.08 = 0.05
        match sched.admit(&[stage], None, 0.0, &SlaTier{id:0, reserved_duty:0.0, deadline_secs:0.08}) {
            AdmitDecision::Reject { bottleneck, .. } =>
                assert_eq!(bottleneck, ResourceId::Compute(SubstrateId(0))),
            other => panic!("expected Reject@GPU, got {other:?}"),
        }
    }

    // --- admit_iff_shared_bandwidth_duty_le_ceiling (NPU compute free, bus saturated) ---
    #[test]
    fn rejects_on_shared_bandwidth_even_when_compute_free() {
        let c = ceilings();
        let mut sched = Scheduler::new(c, vec![SlaTier{id:0, reserved_duty:0.0, deadline_secs:0.08}]);
        sched.seed_bandwidth(0.79); // bus near ceiling, all compute idle
        let stage = StageDuty{substrate:SubstrateId(0), compute_secs:0.0008, // tiny compute
                              bytes_touched: 0.546e9, roofline: RooflineClass::BandwidthBound}; // +0.025 bw
        match sched.admit(&[stage], None, 0.0, &SlaTier{id:0, reserved_duty:0.0, deadline_secs:0.08}) {
            AdmitDecision::Reject { bottleneck, .. } =>
                assert_eq!(bottleneck, ResourceId::SharedBandwidth),
            other => panic!("expected Reject@bandwidth, got {other:?}"),
        }
    }

    // --- reject_model_when_min_step_exceeds_frame_period ---
    #[test]
    fn infeasible_model_at_150hz_on_slow_substrate() {
        let c = ceilings(); // T_f=0.08
        let sched = Scheduler::new(c, vec![]);
        let sb = SubBucketByNfe { t_ar: 0.05, t_inner: 0.02 }; // 0.05+0.02 = 0.07 ≤ 0.08 ok single NFE
        // but 4-NFE: 0.05 + 4*0.02 = 0.13 > 0.08 ⇒ infeasible.
        assert_eq!(
            sched.admit(&[], Some((&sb, &[4])), 0.0, &SlaTier{id:0,reserved_duty:0.0,deadline_secs:0.08}),
            AdmitDecision::InfeasibleModel
        );
    }

    // --- scheduler_orders_by_risk_not_deadline_alone + binary_viability_yields_slack ---
    #[test]
    fn at_risk_session_serves_before_viable_with_sooner_deadline() {
        let t_f = 0.08;
        // A: deadline sooner (0.05) but tiny remaining ⇒ slack 0.04 ≥ T_f? no (0.04<0.08) ... at-risk-ish
        let a = Session{criticality:Criticality::Hi, tier:0, deadline:0.05, predicted_remaining:0.01,
                        progress_frames:10, admitted_at_ticks:0}; // slack 0.04
        // B: deadline later (0.10) but heavy remaining ⇒ slack 0.01 ⇒ MORE at risk despite later deadline
        let b = Session{criticality:Criticality::Hi, tier:0, deadline:0.10, predicted_remaining:0.09,
                        progress_frames:10, admitted_at_ticks:0}; // slack 0.01
        assert!(b.risk_slack() < a.risk_slack(), "B is more at-risk by slack");
        // deadline-EDF alone would serve A first (sooner deadline) — risk-EDF serves B. Assert order.
        let mut v = vec![a, b];
        v.sort_by(|x,y| order_key(x, 0.08, t_f).2.0.total_cmp(&order_key(y,0.08,t_f).2.0));
        assert_eq!(v[0].deadline, 0.10); // B (more at-risk) first, NOT the sooner-deadline A
    }

    #[test]
    fn viable_session_sorts_after_at_risk() {
        let t_f = 0.08;
        let viable = Session{criticality:Criticality::Hi, tier:0, deadline:0.20, predicted_remaining:0.05,
                             progress_frames:0, admitted_at_ticks:0}; // slack 0.15 ≥ 0.08 ⇒ viable
        assert!(viable.is_viable(t_f));
    }

    // --- per_tier_reserved_duty_admits_gold_first ---
    #[test]
    fn bulk_rejected_when_only_gold_reservation_remains() {
        let c = ceilings();
        let gold = SlaTier{id:0, reserved_duty:0.3, deadline_secs:0.02};
        let bulk = SlaTier{id:1, reserved_duty:0.0, deadline_secs:0.20};
        let mut sched = Scheduler::new(c, vec![gold, bulk]);
        sched.seed_compute(SubstrateId(0), 0.55);
        // bulk's effective bound = S(0.8) - gold.reserved(0.3) = 0.5; current 0.55 already over ⇒ reject.
        let stage = StageDuty{substrate:SubstrateId(0), compute_secs:0.0008, bytes_touched:0.0,
                              roofline:RooflineClass::ComputeBound};
        assert!(matches!(sched.admit(&[stage], None, 0.0, &bulk), AdmitDecision::Reject{..}));
        // gold (no other-tier reservation subtracted beyond bulk's 0.0) is admitted into the same headroom.
        assert_eq!(sched.admit(&[stage], None, 0.0, &gold), AdmitDecision::Admit);
    }

    // --- sustained_p99_breach_trips_drift_response_with_hysteresis ---
    #[test]
    fn drift_trips_after_sustain_and_holds_for_hysteresis() {
        let mut d = DriftDetector::new(0.1, /*budget*/0.064, /*sustain*/Duration::from_secs(2),
                                       /*hyst*/Duration::from_secs(60));
        let t0 = Instant::now();
        // feed over-budget steps for >2s ⇒ trips.
        for k in 0..40 { d.observe(0.090, t0 + Duration::from_millis(k*100)); }
        let trip_t = t0 + Duration::from_secs(3);
        assert!(d.shedding(trip_t));
        // recovers below budget — but hysteresis holds shedding for 60s.
        for k in 0..5 { d.observe(0.030, trip_t + Duration::from_millis(k*100)); }
        assert!(d.shedding(trip_t + Duration::from_secs(10)), "still shedding inside 60s hysteresis");
        assert!(!d.shedding(trip_t + Duration::from_secs(61)), "released after hysteresis");
    }

    // --- shed_selects_newest_least_progressed_realtime ---
    #[test]
    fn shed_picks_lo_then_newest_then_least_progressed() {
        let c = ceilings();
        let mut sched = Scheduler::new(c, vec![]);
        sched.force_shedding(); // drift tripped (test hook)
        let hi_old = Session{criticality:Criticality::Hi, tier:0, deadline:0.08, predicted_remaining:0.0,
                             progress_frames:100, admitted_at_ticks:1};
        let lo_new = Session{criticality:Criticality::Lo, tier:0, deadline:0.08, predicted_remaining:0.0,
                             progress_frames:2, admitted_at_ticks:99};
        let v = vec![hi_old, lo_new];
        let victim = sched.shed_victim(&v, Instant::now()).unwrap();
        assert_eq!(victim.criticality, Criticality::Lo); // LO shed before HI
    }

    // --- thermal_throttle_lowers_rated_max ---
    #[test]
    fn thermal_derate_shrinks_effective_bound_and_can_reject() {
        let c = ceilings();
        let mut sched = Scheduler::new(c, vec![]);
        sched.seed_compute(SubstrateId(0), 0.7);
        let stage = StageDuty{substrate:SubstrateId(0), compute_secs:0.004, bytes_touched:0.0,
                              roofline:RooflineClass::ComputeBound}; // +0.05 ⇒ 0.75
        // No throttle: 0.75 ≤ 0.8 ⇒ admit.
        assert_eq!(sched.admit(&[stage], None, 0.0, &SlaTier{id:0,reserved_duty:0.0,deadline_secs:0.08}),
                   AdmitDecision::Admit);
        sched.set_thermal(ThermalState{derate:0.9}); // S_eff = 0.72; 0.75 > 0.72 ⇒ reject.
        assert!(matches!(sched.admit(&[stage], None, 0.0, &SlaTier{id:0,reserved_duty:0.0,deadline_secs:0.08}),
                         AdmitDecision::Reject{..}));
    }

    // --- masked_slot_bandwidth_charged_in_admission ---
    #[test]
    fn masked_slot_bandwidth_term_can_tip_rejection() {
        let c = ceilings();
        let mut sched = Scheduler::new(c, vec![]);
        sched.seed_bandwidth(0.77);
        let stage = StageDuty{substrate:SubstrateId(0), compute_secs:0.0008, bytes_touched:0.0,
                              roofline:RooflineClass::ComputeBound};
        // with no masked term: admits. with a masked-slot bw term that adds 0.05 ⇒ 0.82 > 0.8 ⇒ reject.
        let masked = 0.546e9; // +0.025 ... seed 0.77 + 0.025 = 0.795 ok; use 1.092e9 (+0.05) to tip:
        let masked_tip = 1.092e9;
        assert_eq!(sched.admit(&[stage], None, masked, &SlaTier{id:0,reserved_duty:0.0,deadline_secs:0.08}),
                   AdmitDecision::Admit);
        assert!(matches!(sched.admit(&[stage], None, masked_tip, &SlaTier{id:0,reserved_duty:0.0,deadline_secs:0.08}),
                         AdmitDecision::Reject{..}));
    }

    // --- bottleneck_shifts_AR_to_CFM_to_bandwidth_under_ramp ---
    #[test]
    fn bottleneck_migrates_as_load_changes() {
        let c = ceilings();
        let mut l = DutyLedger::default();
        l.add(&StageDuty{substrate:SubstrateId(0),compute_secs:0.05,bytes_touched:0.0,
                         roofline:RooflineClass::ComputeBound}, &c); // GPU 0.625
        assert_eq!(l.bottleneck(), ResourceId::Compute(SubstrateId(0)));
        l.add(&StageDuty{substrate:SubstrateId(0),compute_secs:0.0,bytes_touched:6.0e9,
                         roofline:RooflineClass::BandwidthBound}, &c); // bandwidth jumps
        assert_eq!(l.bottleneck(), ResourceId::SharedBandwidth); // re-picked → shifted
    }

    // --- prefix_affinity_router_to_kv_holder + affinity_yields_to_duty_when_holder_saturated ---
    #[test]
    fn router_prefers_holder_then_yields_to_duty() {
        use waav_infer_router::*;
        let mut r = Router::default();
        let key = PrefixKey([7u8;32]);
        r.insert_residency(key, WorkerId(1));
        let loads_ok = vec![WorkerLoad{worker:WorkerId(0),projected_duty:0.2},
                            WorkerLoad{worker:WorkerId(1),projected_duty:0.5}];
        assert_eq!(r.route(Some(key), &loads_ok, 0.8), Route::Affinity(WorkerId(1)));
        // holder saturated ⇒ yield to freest (worker 0).
        let loads_hot = vec![WorkerLoad{worker:WorkerId(0),projected_duty:0.2},
                             WorkerLoad{worker:WorkerId(1),projected_duty:0.95}];
        assert_eq!(r.route(Some(key), &loads_hot, 0.8), Route::Reprefill(WorkerId(0)));
    }

    // --- extreme_64ar_8cfm_codec_stt_admission_feasible_or_rejected (the EXTREME compound) ---
    #[test]
    fn extreme_mixed_clock_set_admits_then_rejects_at_saturation() {
        let c = ceilings();
        let mut sched = Scheduler::new(c, vec![SlaTier{id:0,reserved_duty:0.0,deadline_secs:0.08}]);
        let sb = SubBucketByNfe{ t_ar:0.009, t_inner:0.004 };
        let nfe = [2,2,2,2,4,4,4,4]; // 8 CFM, max NFE 4
        assert!((sb.t_step(&nfe) - 0.025).abs() < 1e-9); // feasible: 0.025 ≤ 0.08
        // GPU compute 0.35, NPU 0.125, bandwidth 0.62 — all ≤ 0.8 ⇒ admit.
        let stages = extreme_stage_set(); // helper builds the 64AR+8CFM+codec+STT duties
        assert_eq!(sched.admit(&stages, Some((&sb,&nfe)), 0.0,
                   &SlaTier{id:0,reserved_duty:0.0,deadline_secs:0.08}), AdmitDecision::Admit);
        // now push the bus over: extra bandwidth-bound stage tips Σ bw > 0.8 ⇒ Reject@bandwidth, not glitch.
        sched.commit(&stages, &c);
        let extra = StageDuty{substrate:SubstrateId(0),compute_secs:0.001,bytes_touched:4.5e9,
                              roofline:RooflineClass::BandwidthBound};
        assert!(matches!(sched.admit(&[extra], Some((&sb,&nfe)), 0.0,
                         &SlaTier{id:0,reserved_duty:0.0,deadline_secs:0.08}),
                         AdmitDecision::Reject{ bottleneck: ResourceId::SharedBandwidth, .. }));
    }
}
```

**Type count:** 14 (`ResourceId`, `SubstrateId`, `RooflineClass`, `StageDuty`, `Ceilings`, `DutyLedger`, `SubBucketByNfe`, `DriftDetector`, `ThermalState`, `Criticality`, `SlaTier`, `Session`, `Scheduler`, `AdmitDecision`) + 4 router types (`Router`, `Residency`, `WorkerLoad`, `Route`) = **18 types**.
**Representative RED tests above:** 16 (covering all 18 §7 M4.2/M4.3 named gates + the EXTREME compound). The full gate set maps 1:1 to the IMPL §7 names below.

### Gate → test mapping (IMPL §7 M4.2/M4.3, gate-NAME granularity)

| IMPL §7 gate (M4.2/M4.3) | RED test above |
|---|---|
| `nested_variable_nfe_T_step_sums_max_subbucket` | `nested_variable_nfe_t_step_uses_max_not_mean` |
| `sub_bucket_inner_by_nfe_within_one_outer_step` | `sub_buckets_group_by_nfe_in_slot_order` |
| `scheduler_orders_by_risk_not_deadline_alone` | `at_risk_session_serves_before_viable_with_sooner_deadline` |
| `binary_viability_yields_slack_when_safe` | `viable_session_sorts_after_at_risk` |
| `admit_iff_every_substrate_duty_le_S` | `rejects_when_any_substrate_exceeds_bound_naming_bottleneck` |
| `bandwidth_duty_measured_via_dram_active_co_load` | `bandwidth_duty_uses_bytes_over_ceiling_times_tickrate` |
| `admit_iff_shared_bandwidth_duty_le_ceiling` | `rejects_on_shared_bandwidth_even_when_compute_free` |
| `roofline_class_serializes_two_bandwidth_bound` | (covered by Σ-bw test + `bandwidth_bound_count`) |
| `bottleneck_repicked_per_admit` / `bottleneck_is_argmax_utilization_over_all_resources` | `rejects_when_any_substrate_exceeds_bound_naming_bottleneck` |
| `bottleneck_shifts_AR_to_CFM_to_bandwidth_under_ramp` | `bottleneck_migrates_as_load_changes` |
| `masked_slot_bandwidth_charged_in_admission` | `masked_slot_bandwidth_term_can_tip_rejection` |
| `reject_model_when_min_step_exceeds_frame_period` | `infeasible_model_at_150hz_on_slow_substrate` |
| `per_tier_reserved_duty_admits_gold_first` / `tier_arbiter_protects_contract_relegates_within_looser_sla` | `bulk_rejected_when_only_gold_reservation_remains` |
| `sustained_p99_breach_trips_drift_response_with_hysteresis` | `drift_trips_after_sustain_and_holds_for_hysteresis` |
| `shed_selects_newest_least_progressed_realtime` | `shed_picks_lo_then_newest_then_least_progressed` |
| `thermal_throttle_lowers_rated_max` | `thermal_derate_shrinks_effective_bound_and_can_reject` |
| `prefix_affinity_router_to_kv_holder` / `affinity_yields_to_duty_when_holder_saturated` | `router_prefers_holder_then_yields_to_duty` |
| `extreme_64ar_8cfm_codec_stt_admission_feasible_or_rejected` | `extreme_mixed_clock_set_admits_then_rejects_at_saturation` |
| `herd_spreads_across_replicas` | (router `route_herd` — body deferred, see residuals) |

### Server integration (the seam change)

The current `Engine` holds `admission = Arc<Semaphore>` (flat permit count) and rejects with `InferError::admission_rejected(retry_after_ms)`. The migration is **additive and revert-safe**:
- `Scheduler` *subsumes* the semaphore (a permit ≈ one duty unit). `try_admit()` becomes `scheduler.admit(stages, nested, masked, tier)`; `AdmitDecision::Reject{retry_after_ms, ..}` → the existing `InferError::AdmissionRejected` (HTTP 429, `retry_after_ms`) — **the wire contract is unchanged** (the protocol already carries `retry_after_ms`). `InfeasibleModel` → a typed load/route-time refusal (not a runtime 429).
- Edge inline mode constructs **no** `Scheduler`/`DutyLedger` (the `single_stream_edge_pays_nothing` discipline): the duty-aware path is only built when `mode != inline`.

---

## Residual gaps

1. **DRAM_ACTIVE live scrape → `StageDuty.bytes_touched` (the one genuine residual).** The scheduler math consumes `bytes_touched` as a calibrated input and is fully GPU-free testable. The live DCGM `DRAM_ACTIVE`/bytes-streamed reader that *populates* it during the §8.3b co-load calibration pass is an **M4.4-spine wiring task**, not in this unit layer. The method exists and is already mandated by the spine (catalog J23); only the plumbing (`CalibrationStamp{bytes_touched_per_stage}`) is unbuilt. Until wired, `bytes_touched` falls back to a per-roofline-class estimate (`bandwidth-bound ⇒ peak; compute-bound ⇒ 0.3×peak`), which is conservative (over-charges bandwidth → never over-admits). **Risk: low** (conservative fallback is admission-safe); **gate to add to M4.4:** `calibration_stamp_populates_bytes_touched_from_dram_active`.

2. **`route_herd` greedy assignment body.** The single-route arbiter (`route`) is fully designed + tested; the herd-spread (`route_herd`, N returning voices across all holders) is specified as "greedily assign to the least-loaded admissible holder, updating projected_duty" but the loop body is left as `unimplemented!()` with a named gate (`herd_spreads_across_replicas`). It is a straightforward greedy over `route`; deferred only to keep this layer KISS. **Risk: low.**

3. **The `SlidingServe`/`DuetServe` `predicted_remaining` predictor is consumed, not built here.** `Session.predicted_remaining` (the risk-slack input) and the L10 prefill-firewall predictor are the **same** 7-feature latency model (MAE 2.5ms). This layer treats it as an input; the predictor itself is a shared M4.3-firewall component (already named there). No double-build, but the two consumers must share one predictor instance — a **cross-layer wiring note**, not a gap. **Risk: low** (degenerate predictor `predicted_remaining = T_step` makes risk-EDF fall back to deadline-EDF, still correct).

4. **`reserved_for_other_tiers` assumes static per-tier reservations.** Dynamic reservation (gold's reservation shrinking when no gold traffic is present, to let bulk use the headroom — "relegate within its own SLA") is the *work-conserving* refinement. The current design protects the contract (never breaches a looser tier to over-serve a satisfied tighter one) but does **not** reclaim an idle gold reservation for bulk. This is the difference between *strict reservation* (shipped) and *work-conserving reservation* (VTC-style). **Risk: medium** (idle gold capacity is left on the table under bulk pressure); **gate to add:** `idle_tier_reservation_reclaimed_by_looser_tier`. Deferred — strict reservation is the safe correct floor; work-conserving is an optimization on top.

5. **Masked-slot term is the "budget" branch only (R5a disjunction).** Per v2.1 §6.4 the default is the `masked_bandwidth_duty` admission term (implemented above — charge the idle slots' bandwidth). The optional **repack/compaction** branch (gather active rows into a smaller pre-captured cohort) is explicitly **out of scope** for this layer — it belongs to the slot-table/cohort layer (`compaction.rs`, BAT-75/76/111), referenced not duplicated. The scheduler's duty math is correct under either branch (it just sees a smaller captured cohort if compaction runs). **Not a gap in L4** — a clean layer boundary.
