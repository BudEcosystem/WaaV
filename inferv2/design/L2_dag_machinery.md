# L2 — DAG machinery (full design)

**Status:** design · **Date:** 2026-06-17 · **Layer:** `INFER_ENGINE_V2.md` §6.2 (gate-name granularity) → **here, in full** · **Substrate:** the unbuilt `waav-infer-runtime` crate (IMPL §1), atop the live 6-crate codebase.

> This completes ONE layer of v2.1 to "in full": the conditional/fan-in/multi-terminal DAG machinery, FINAL-after-tail-drain propagation, `SentenceAggregator`/`StableSpanGate`, `DagSlotReset` + DAG-wide `channel_id`, `CloudStage`, and the reliable ACK'd DAG-wide barge-in. v2.1 §6.2 named these at gate-name granularity; the coverage audits (`10_features`, `03_s2s`) flagged them as the **single largest cluster of PARTIAL/GAP** (structural holes #1/#3, GAPs FEAT-29/49/55/60, S2S ROOT-GAP-E). This document gives the actual Rust types, algorithms, and RED test bodies, then closes with an adversarial residual-gap pass.
>
> **Scope discipline (what L2 is NOT):** L2 is the *composition glue* over a **static topology of stage nodes**. It does NOT define the feature-stage taxonomy (that is L1 / `StageState`), the duplex multi-stream seam (L3), the scheduler objective (L4), or the reasoning cascade internals (L6). L2 *consumes* `StageState::reset(slot)` (L1) and `ArStepModel::reset_slot` (M2.3) and *feeds* `SentenceAggregator` output into the L6 cascade. The seams to those layers are named explicitly in §(b).7 and §(c).

---

## (a) Convergence table

For each prev-PARTIAL/GAP DAG scenario: does the **deep design below** (not just the v2.1 gate-name) close it? `CLOSED` = a named type + algorithm + RED test here covers the scenario's twist. `RESIDUAL` = a real piece still missing after this design (carried to §(c)).

| Scenario (coverage id) | prev | v2.1 §6.2 gate-name | THIS design closes via | verdict |
|---|---|---|---|---|
| Conditional branch / VAD-gate / langID-route (FEAT-5/7/43, S2S-56) | PARTIAL | `route_fn_returns_in_static_topology` | `StageNode.route_fn: RouteFn` returns `RouteTarget` ∈ static `outputs_to`, `Drop`→terminal-sink, empty forbidden; §(b).1 algorithm + 3 RED tests | **CLOSED** |
| Fan-in deadlock on conditional branch (FEAT-21/33, S2S-56) | PARTIAL | `wait_for_fn_conditional_branch_no_deadlock` | `StageNode.wait_for_fn: WaitForFn` computes `ExpectedSources` per-request from req flags; join releases on expected-set, not declared-set; §(b).2 + 2 RED tests | **CLOSED** |
| Multi-terminal text+audio, per-terminal FINAL (FEAT-20/44/45, S2S-2) | PARTIAL | `multi_terminal_join_by_time` | `Terminals(Vec<TerminalId>)` + `JoinByTime` merge node; `request_narrows_terminals`; per-terminal independent FINAL; §(b).3 + 3 RED tests | **CLOSED** |
| FINAL propagation after per-stage tail drain (FEAT-16/27, S2S-5) | PARTIAL | `final_propagates_after_tail_drain_per_terminal` | `EdgeSignal::Final{from_stage, after_tail_drain}` + `FinalGate` (stage flushes its F5 marker-heap / delay tail before forwarding); §(b).4 + 2 RED tests | **CLOSED** |
| `cancelled ≠ completed` through the whole DAG (FEAT-16/30, S2S-7) | PARTIAL | `cancelled_distinct_from_final_through_dag` | `TerminalFrame{Final\|Cancelled\|Error}` rides every edge; each stage forwards the *kind*; §(b).4 + 1 RED test | **CLOSED** |
| Sentence/stable-span aggregation (FEAT-8/9/17/36, S2S-26) | PARTIAL/GAP | `sentence_aggregator_commits_on_boundary` | `SentenceAggregator` trait + `StableSpanGate` boundary algorithm; commits committed-span only, O(N) not O(N²); §(b).5 + 3 RED tests | **CLOSED** |
| DAG-wide transactional slot reset (FEAT-55 GAP, FEAT-54/64/69) | GAP | `dag_slot_reset_fans_to_all_stages` | `DagSlotReset` transaction: fans `StageState::reset(slot)` to every stage + `ArStepModel::reset_slot` + bumps `ChannelId`; §(b).6 + 2 RED tests | **CLOSED** |
| Stale-occupant late output dropped at every stage (FEAT-55, S2S-21) | GAP | `dag_channel_id_drops_stale_occupant_output` | DAG-wide `ChannelId[slot]` stamped on every frame; each stage egress drops frames with stale id; §(b).6 + 1 RED test | **CLOSED** |
| Vendor-mixed / cloud stage (FEAT-29/49/52 GAP, S2S-15) | GAP | `cloud_stage_remote_cancel_and_failfast` | `CloudStage` trait (`paradigm=remote`): own network-SLO budget, credit relay, hard-timeout→`Error` fan-out, `cancel_remote(channel)` hook; §(b).7 + 3 RED tests | **CLOSED** |
| DAG-wide barge-in, reliable per-stage ACK (FEAT-30/49, S2S-18/82) | PARTIAL | `barge_in_fans_out_to_all_stages_with_ack` | `BargeIn` fans one cancel to ALL stages, awaits per-stage `CancelAck` (bounded), reconciles, NOT fire-and-forget; §(b).8 + 2 RED tests | **CLOSED** |
| Per-span code-switch re-route (FEAT-60 GAP, S2S-66) | GAP | (none — out of §6.2) | route_fn re-evaluated per **span** boundary (not per request); but MT re-aggregation depends on L1 langID stage | **RESIDUAL** (§(c).1) |
| Time-aligned join (diarize+STT) (FEAT-18/24/41/65) | PARTIAL | `multi_terminal_join_by_time` | `JoinByTime` aligns by frame-stamp; but *silent-speaker-doesn't-stall* needs L3 active-set | **RESIDUAL** (§(c).2) |

**Tally: 9 CLOSED, 2 RESIDUAL.** (The 2 residuals are cross-layer: they require an L1 langID feature-stage and the L3 duplex active-set respectively — L2 provides the routing/join *machinery*, the missing piece is the *upstream signal* that lives in another layer. Both are named with their seam in §(c).)

---

## (b) Deep design

The whole layer lands in **`waav-infer-runtime/src/dag/`** (a new sub-module of the IMPL §1 runtime crate). It is **pure logic, no backend deps** (testable with `NopStage` doubles exactly as `model.rs` uses `NopLoader`). It builds on the static `StageNode` schema from `INFER_ENGINE.md` §3.1 by **adding fields**, never replacing them — so the existing 3-node CosyVoice2 / 2-node dots.tts DAGs (M3 accept) keep working with all new fields defaulted to their linear-pass behavior.

### Shared newtypes & errors (match the crate idiom)

```rust
// waav-infer-runtime/src/dag/ids.rs
// Newtype IDs (the model.rs / protocol style: small Copy newtypes, never bare usize on the wire).

/// A fixed lockstep slot index (0..B_max). Shared with the scheduler's SlotTable (M2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SlotId(pub u32);

/// A stage node's stable id within ONE static topology (manifest-declared).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StageId(pub u16);

/// A DAG terminal (egress) id. A multi-terminal DAG has >1 (e.g. {transcript, audio}).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerminalId(pub u16);

/// Monotonic per-slot occupant epoch (catalog F3 `channel_id`, here LIFTED DAG-WIDE).
/// Stamped on every frame; a stage drops a frame whose id != the slot's live ChannelId.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChannelId(pub u64);
impl ChannelId {
    pub fn bump(self) -> Self { ChannelId(self.0 + 1) }
}
```

All fallible DAG operations return the existing `waav_infer_protocol::InferError`. L2 reuses the closed `ErrorCode` set — **no new error codes** (the audit's KISS rule). Routing/wait-for misconfiguration is `ErrorCode::BadConfig` (it's a manifest-topology bug, caught at validate-time, 400-class); a stalled cloud node is `ErrorCode::StallTimeout` (already retriable, 503); a downstream-full park that times out is `ErrorCode::Backpressure`.

### The extended `StageNode` (additive fields over `INFER_ENGINE.md` §3.1)

```rust
// waav-infer-runtime/src/dag/node.rs
use waav_infer_protocol::InferError;

/// Per-request routing: pick the SINGLE downstream target for `data` leaving `from`.
/// MUST return a target whose StageId/TerminalId is in `from`'s static `outputs_to`,
/// or `Drop`. Returning an off-topology target is a BadConfig at validate-time
/// (route_fn results are validated against the static topology before the DAG runs;
///  see `validate_topology`). Boxed so a manifest/closure can supply it; Send for the
/// per-tick driver. (Catalog G11: "route_fn must return ∈ statically-declared next".)
pub type RouteFn =
    Box<dyn Fn(&RequestCtx, /*data*/ &Payload) -> RouteDecision + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteDecision {
    To(StageId),
    Terminal(TerminalId),
    /// Explicit terminal SINK — the ONLY legal "no downstream" (catalog G11: empty route forbidden;
    /// a silent VAD frame routes here, NOT to `[]` which would deadlock a waiting join).
    Drop,
}

/// Per-request fan-in: which upstream sources THIS request will actually produce.
/// A join releases when it has collected exactly this set — NOT the static `inputs[]`
/// (catalog G11: fixed `wait_for=[a,b,c]` deadlocks when a branch never fires).
pub type WaitForFn = Box<dyn Fn(&RequestCtx) -> ExpectedSources + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedSources(pub smallvec::SmallVec<[StageId; 4]>);

/// Multi-terminal: a stage that is a fan-out point declares its terminal set; a request
/// may NARROW it (text-only request drops the audio terminal). Per-terminal FINAL.
#[derive(Debug, Clone, Default)]
pub struct Terminals(pub smallvec::SmallVec<[TerminalId; 2]>);

/// The additive DAG fields. ALL default to the linear-pass behavior so the existing
/// static `inputs[]`/`outputs_to[]` 3-node DAGs are unchanged (Option/None = "linear").
#[derive(Default)]
pub struct DagFields {
    /// None  ⇒ linear pass-through to the single `outputs_to` (today's behavior).
    /// Some  ⇒ conditional routing.
    pub route_fn: Option<RouteFn>,
    /// None  ⇒ wait for ALL static `inputs[]` (today's behavior, valid for an unconditional join).
    /// Some  ⇒ dynamic per-request expected set.
    pub wait_for_fn: Option<WaitForFn>,
    /// Empty ⇒ single implicit terminal. Non-empty ⇒ multi-terminal fan-out node.
    pub terminals: Terminals,
    /// How a fan-in node merges its collected inputs.
    pub join: JoinPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JoinPolicy {
    /// Forward each input as it arrives, no merge (default — a pure pass / route node).
    #[default]
    Passthrough,
    /// Collect the per-request expected set, release once complete (the fan-in node).
    OnExpectedSet,
    /// Time-aligned merge by frame-stamp (diarize+STT). Holds a bounded reorder window.
    ByTime { window_frames: u32 },
}
```

`RequestCtx` is the per-request immutable flag bag the route/wait closures read (`is_text_only`, `target_lang`, `enable_diarize`, `enable_sentiment`, …). It is `Arc`-shared (catalog G5: fan-out clones the container, shares `Arc` on immutable leaves — no serialize).

### (b).1 `route_fn` — conditional routing constrained to static topology

**Algorithm.** Two phases. (1) **validate-time** (`validate_topology`, run once when the DAG is built from the manifest, before any request): for every node with a `route_fn`, the manifest declares the node's `outputs_to` *superset* of all reachable targets; the validator asserts `route_fn`'s declared candidate set ⊆ `outputs_to ∪ {Drop}`. Topology stays statically analyzable (no node loads a model that the static graph didn't reserve VRAM for — ties the J2 VRAM accountant: all candidate branches are in the static residency set). (2) **run-time** (per frame, in the per-tick driver): call `route_fn(ctx, &payload)`; check the returned `RouteDecision` against `from`'s static `outputs_to`; an off-topology target is an `InferError::bad_config` (defense in depth — even though validate-time should have caught it, a buggy dynamic closure that computes a target from `payload` is caught here, never silently dropped). `Drop` routes to the terminal sink. Empty/`None` from a closure that was supposed to route is a `BadConfig` (catalog G11: forbid empty).

```rust
// waav-infer-runtime/src/dag/route.rs
impl StageNode {
    /// Resolve the downstream for `payload` leaving this node. Returns the validated target.
    pub fn route(&self, ctx: &RequestCtx, payload: &Payload) -> Result<RouteDecision, InferError> {
        let decision = match &self.dag.route_fn {
            None => RouteDecision::To(self.single_output()?),     // linear pass (today)
            Some(f) => f(ctx, payload),
        };
        match &decision {
            RouteDecision::To(target) if !self.outputs_to.contains(target) =>
                Err(InferError::bad_config(format!(
                    "route_fn for stage {:?} returned off-topology target {:?}", self.id, target))),
            RouteDecision::Terminal(t) if !self.declares_terminal(*t) =>
                Err(InferError::bad_config(format!(
                    "route_fn for stage {:?} returned undeclared terminal {:?}", self.id, t))),
            _ => Ok(decision),  // To(valid) | Terminal(valid) | Drop are all legal
        }
    }
}
```

### (b).2 `wait_for_fn` — dynamic fan-in (the deadlock fix)

**Algorithm.** A join node accumulates inputs into a per-(request,slot) `JoinBuffer`. On the *first* input for a request, it computes `expected = wait_for_fn(ctx)` once and caches it (a request's expected set is fixed for the request lifetime — a text-only request never grows an audio source mid-flight; that re-evaluation case is the per-span residual §(c).1, not this node). The join releases when `collected == expected` (set equality on `StageId`). It must NOT wait on the static `inputs[]` superset (that is the G11 deadlock). A `Drop` upstream that the request *did* expect is reconciled by `route_fn` already having narrowed the source — i.e. an expected source can never legitimately `Drop` for that request (validate-time invariant: a source in any `wait_for_fn` result must have a route to the join). Bookkeeping is **capped** (catalog G6): a `JoinBuffer` is freed on release AND on slot reset; the live-buffer count is gauged.

### (b).3 multi-terminal + `JoinByTime`

**Multi-terminal.** A fan-out node carries `Terminals`. A request may narrow it: `effective_terminals = node.terminals ∩ ctx.requested_terminals` (text-only → `{transcript}`). Each terminal gets its **own** `TerminalFrame::Final` when ITS stream drains — FINAL on the transcript terminal does NOT close the audio terminal (the FEAT-44 `If mishandled`). The DAG completes a request only when *all effective terminals* have emitted FINAL or Cancelled.

**`JoinByTime`.** For diarize+STT-style merges, the join holds a bounded reorder window keyed by frame-stamp, emits in time order once both branches have delivered up to a watermark frame. `window_frames` bounds memory (catalog G6). The merge is on **frame-stamp**, distinct from `OnExpectedSet` which is on **source identity** — both are needed (FEAT-18 needs *which sources* AND *temporal alignment*; `OnExpectedSet` answers the first, `ByTime` the second).

### (b).4 FINAL as a DAG-propagated edge signal (after tail drain) + cancelled≠completed

**The edge enum.** Every typed inter-stage channel (`INFER_ENGINE.md` §3.1) now carries, in-band, an `EdgeSignal` alongside its data payload:

```rust
// waav-infer-runtime/src/dag/signal.rs
/// What rides a typed inter-stage edge, in-band (catalog G2: end-of-stream is an EXPLICIT
/// frame, never inferred from silence; cancelled ≠ completed).
pub enum EdgeSignal {
    /// A data frame (codec tokens / latent chunk / whole tensor), stamped with the slot's ChannelId.
    Data { payload: Payload, channel: ChannelId, frame_idx: u64 },
    /// FINAL from `from_stage`. `after_tail_drain=false` is a topology error at a stage that has
    /// a delay tail (validate-time): a stage with an F5 marker-heap / acoustic-delay ring MUST
    /// set true (it flushed its tail). A pass-through stage with no tail forwards the upstream value.
    Final { from_stage: StageId, channel: ChannelId, after_tail_drain: bool },
    /// Barge-in / deadline-unreachable cancellation — DISTINCT terminal from Final (catalog G2).
    Cancelled { from_stage: StageId, channel: ChannelId, reason: CancelReason },
    /// A terminal failure (cloud disconnect, NaN-after-reject-budget, op-fault). Fans to all
    /// downstream + cancels the whole request (catalog G9 fail-fast).
    Error { from_stage: StageId, channel: ChannelId, err: InferError },
}

/// The terminal frame the egress emits to the client — one per terminal.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalFrame {
    Final { terminal: TerminalId },
    Cancelled { terminal: TerminalId, reason: CancelReason },
    Error { terminal: TerminalId, err: InferError },
}
```

**The `FinalGate` algorithm (per stage egress).** A stage forwards `Final` downstream **only after its own delay tail has drained**. Concretely: a stage with an F5 future-step marker-heap (`marker.rs`, M2.4) or an acoustic-delay ring (F8, M2.3) holds the incoming `Final{after_tail_drain:false}`, keeps stepping until its marker fires / ring flushes (offset ≥ real_end, catalog F5), then emits `Final{from_stage:self, after_tail_drain:true}` downstream. A pass-through stage (no tail) forwards immediately. This makes FINAL traverse N chained stages correctly (the FEAT-27 headline; F5 was single-stage-only). `Cancelled`/`Error` do NOT wait for tail drain — they abort the tail (a cancelled stream's leftover audio is discarded, not flushed — that is the whole point of cancelled≠completed).

```rust
// waav-infer-runtime/src/dag/final_gate.rs
impl StageEgress {
    /// Called when an upstream FINAL arrives. Returns Some(signal) to forward NOW, or None to hold
    /// until the tail drains (the driver re-polls drain each tick).
    pub fn on_upstream_final(&mut self, up: EdgeSignal) -> Option<EdgeSignal> {
        match up {
            EdgeSignal::Final { channel, .. } => {
                if self.tail_drained(channel) {
                    Some(EdgeSignal::Final { from_stage: self.id, channel, after_tail_drain: true })
                } else {
                    self.pending_final.insert(channel);     // hold; driver re-polls drain
                    None
                }
            }
            // Cancelled/Error abort the tail and propagate immediately (cancelled ≠ completed).
            other => { self.abort_tail(other.channel()); Some(other.restamped(self.id)) }
        }
    }
    /// Driver re-poll: emit a held FINAL once the tail finally drains.
    pub fn poll_drain(&mut self, channel: ChannelId) -> Option<EdgeSignal> {
        if self.pending_final.remove(&channel) && self.tail_drained(channel) {
            Some(EdgeSignal::Final { from_stage: self.id, channel, after_tail_drain: true })
        } else { None }
    }
}
```

### (b).5 `SentenceAggregator` / `StableSpanGate`

**The trait + the boundary algorithm.** The aggregator buffers delta text (STT partials, or LLM token stream) and emits ONLY committed spans on a sentence/clause boundary — the antidote to O(N²) MT re-translation and casing flicker (FEAT-8/9/17/36, S2S-26).

```rust
// waav-infer-runtime/src/dag/aggregate.rs
/// Commits a stable span when a boundary is reached. The cascade LLM→TTS and STT→MT legs
/// consume committed spans, never the growing transcript (O(N), not O(N²)).
pub trait SpanAggregator: Send {
    /// Feed an incremental delta (NOT cumulative). Returns the spans that just became COMMITTED
    /// (stable) — usually empty, occasionally one or more sentences/clauses.
    fn push(&mut self, delta: &str) -> smallvec::SmallVec<[CommittedSpan; 2]>;
    /// On FINAL: flush the trailing uncommitted buffer as one last span (the tail clause that
    /// never got a terminal punctuation — else the last words are dropped).
    fn flush(&mut self) -> Option<CommittedSpan>;
    /// On slot reset (DagSlotReset): drop the buffer (cross-user privacy — F3).
    fn reset(&mut self);
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommittedSpan { pub text: String, pub start_char: usize }

/// The default: commit on terminal punctuation OR a stability window (a span unchanged for
/// `stable_after` deltas is committed even without punctuation — handles streaming STT that
/// revises earlier words; catalog "LocalAgreement"/partial-stability, L1 AsrFeaturePost).
pub struct StableSpanGate {
    buf: String,
    committed_to: usize,        // char offset already emitted
    boundary: BoundaryRule,     // SentenceTerminator | ClauseOrSentence | Custom(fn)
    stable_after: u8,           // deltas a tail-span must be unchanged before forced-commit
    unchanged_count: u8,
}
```

**Boundary algorithm.** On `push(delta)`: append to `buf`; scan `buf[committed_to..]` for the **last** boundary char per `boundary` (period/?/!/… for `SentenceTerminator`; also `,;:` for `ClauseOrSentence`) that is followed by whitespace-or-end (avoids splitting "3.14" / "Mr."). If found at offset `k`: commit `buf[committed_to..k]` as a `CommittedSpan`, advance `committed_to=k`, reset `unchanged_count`. If no boundary but the tail span is byte-identical to the previous `push`'s tail (a streaming STT that stopped revising), increment `unchanged_count`; at `stable_after`, force-commit the stable prefix (so MT isn't starved when the speaker pauses mid-sentence). The committed text is **never re-emitted** (the O(N) guarantee). This is deliberately KISS — no ML boundary model; a `Custom(fn)` hook is the escape for languages without western punctuation (the L1 unicode-script segmenter can supply it).

### (b).6 `DagSlotReset` transaction + DAG-wide `ChannelId`

**The transaction.** `DagSlotReset` is the orchestrator that FEAT-55 found missing: one `reset_slot(slot)` that fans to EVERY stage's per-slot state (denoise gain, STT word buffers, MT/aggregator context, codec window, inner-solver latent, KV, sampler RNG) and bumps the DAG-wide `ChannelId[slot]` so any in-flight late output from the previous occupant is dropped at every stage. It composes the per-AR-model `ArStepModel::reset_slot` (M2.3) AND the per-feature-stage `StageState::reset(slot)` (L1) AND the aggregator `reset()` — these are the *participants*; `DagSlotReset` is the *transaction* over them.

```rust
// waav-infer-runtime/src/dag/slot_reset.rs
/// A stage's per-slot resettable state. EVERY stage type implements this (not just ArStepModel).
/// Feature stages (denoise/AGC/diarize/codec-window/aggregator) implement it via L1's StageState;
/// AR stages delegate to ArStepModel::reset_slot. This is the contract FEAT-55 was missing.
pub trait SlotResettable: Send {
    fn reset_slot(&mut self, slot: SlotId);
}

/// The DAG-wide transactional reset. Ordering: bump ChannelId FIRST (so any frame produced
/// during the reset window is already stale and will be dropped), THEN fan reset to every stage.
pub struct DagSlotReset<'a> {
    stages: &'a mut [Box<dyn SlotResettable>],
    channel_ids: &'a mut [ChannelId],   // indexed by SlotId
}
impl DagSlotReset<'_> {
    pub fn reset(&mut self, slot: SlotId) {
        // 1. Invalidate the previous occupant: bump epoch BEFORE clearing state, so a frame the
        //    old occupant produced mid-reset is stale-stamped and dropped downstream (no race).
        let i = slot.0 as usize;
        self.channel_ids[i] = self.channel_ids[i].bump();
        // 2. Fan the reset to EVERY stage transactionally (denoise gain, word buffers, MT ctx,
        //    codec window, inner latent, KV, RNG). reset_slot is infallible per F3 (mask-based,
        //    no byte-wipe) so the transaction can't half-fail.
        for stage in self.stages.iter_mut() {
            stage.reset_slot(slot);
        }
    }
    pub fn live_channel(&self, slot: SlotId) -> ChannelId { self.channel_ids[slot.0 as usize] }
}
```

**Stale-drop at every stage egress.** Every `EdgeSignal` is stamped with the producing slot's `ChannelId` at emit. A stage's ingress drops any `EdgeSignal` whose `channel` `<` the slot's live `ChannelId` (the previous occupant's frame, in flight when the reset happened). This is the catalog F3 monotonic `channel_id` guard, lifted from AR-model-local to **DAG-wide** — the FEAT-55 cross-user contamination guard "at every stage."

```rust
// drop guard, called at every stage ingress
fn accept(&self, sig: &EdgeSignal, live: ChannelId) -> bool {
    sig.channel() >= live   // stale (< live) frame from the prev occupant ⇒ drop silently
}
```

### (b).7 `CloudStage` (vendor-mixed / remote node)

A `CloudStage` is a stage whose work runs on a **remote vendor endpoint** (cloud STT/MT/TTS/S2S), not a local model. It is the FEAT-29/49/52 GAP. It is NOT in the local duty ledger (its latency is network, not GPU) but it IS a DAG node with the same edge contract — so FINAL/cancelled/error propagate through it like any stage.

```rust
// waav-infer-runtime/src/dag/cloud.rs
/// A stage backed by a remote vendor session (paradigm=remote). It rides the same EdgeSignal
/// edges but its SLO and failure model are network, not GPU. (Coverage holes FEAT-29/49/52.)
pub trait CloudStage: Send {
    /// Open a remote streaming session for this slot; returns a handle used for cancel.
    fn open(&mut self, slot: SlotId, ctx: &RequestCtx) -> Result<RemoteSession, InferError>;
    /// Relay one upstream frame to the remote, credit-windowed (catalog G4: bounded credits,
    /// notify-before-wait). Returns downstream frames produced, or Backpressure if no credit.
    fn relay(&mut self, slot: SlotId, up: EdgeSignal) -> Result<Vec<EdgeSignal>, InferError>;
    /// RELIABLE cancel to the remote session on barge-in (catalog G9, FEAT-49): abort the cloud
    /// WS/session, not just local stages. Idempotent; ACKs the DAG-wide barge-in (§(b).8).
    fn cancel_remote(&mut self, slot: SlotId) -> Result<(), InferError>;
    /// The network-SLO budget for the firewall/watchdog (LARGER than a local stage; excluded
    /// from the local compute duty ledger — it's a network hop, not a GPU tick).
    fn network_slo(&self) -> NetworkSlo;
}
```

**Fail-fast.** A `CloudStage` has a hard request-timeout (`network_slo().deadline`). On disconnect or timeout-with-no-FINAL, it emits `EdgeSignal::Error{from_stage, err: StallTimeout}` which fans to all downstream stages and cancels the request (catalog G9 fail-fast, FEAT-29 `If mishandled`: "stalled cloud MT blocks DAG with no FINAL"). On barge-in, the DAG-wide cancel (§(b).8) invokes `cancel_remote` and awaits its ACK like any other stage — closing the FEAT-49 "cloud keeps streaming after barge-in" hole.

### (b).8 DAG-wide barge-in with reliable per-stage ACK

**The cancel fan-out.** One barge-in (or deadline-unreachable, or disconnect) fans `Cancel{slot, channel}` to **all** stages of the DAG and **awaits a `CancelAck` from each** (bounded by a per-stage deadline). This is catalog G9: NOT fire-and-forget PUB/SUB (which drops to a late-connecting stage → a stream that keeps speaking). A reliable acked channel + a reconcile pass that alarms if a stage doesn't ACK in time (and force-frees its slot).

```rust
// waav-infer-runtime/src/dag/barge_in.rs
/// One cancel → fanned to ALL stages with a per-stage ACK (catalog G9; NOT fire-and-forget).
pub struct BargeIn<'a> { stages: &'a [StageHandle] }
impl BargeIn<'_> {
    /// Returns Ok once EVERY stage has ACKed within `deadline`; Err(StallTimeout) names the
    /// stage(s) that didn't ACK (the leak-reconciler force-frees those slots — J15/F9).
    pub fn cancel_all(&self, slot: SlotId, channel: ChannelId, deadline: Duration)
        -> Result<(), InferError>
    {
        let mut acked = bitvec![0; self.stages.len()];
        for (i, st) in self.stages.iter().enumerate() {
            st.send_cancel(Cancel { slot, channel });      // reliable channel, not PUB/SUB
        }
        // Await ACKs up to the deadline; a CloudStage's ACK arrives after cancel_remote returns.
        self.collect_acks(&mut acked, deadline);
        if acked.all() { Ok(()) }
        else {
            let missing: Vec<_> = (0..self.stages.len()).filter(|&i| !acked[i]).collect();
            Err(InferError::new(ErrorCode::StallTimeout,
                format!("barge-in: stages {missing:?} did not ACK; slots force-freed")))
        }
    }
}
```

The barge-in is what *triggers* a `DagSlotReset` (§(b).6) once all stages ACK — cancel-then-reset is one ordered sequence, so the next occupant of the slot sees no residual (ties FEAT-55 + FEAT-30). A barge-in **storm** (S2S-82: 20 cancels in 1s) is bounded by the same per-stage ACK channel being a bounded queue — cancels are processed at the tick rate, never stalling the cohort loop (the cancel handler runs at the frame boundary, catalog J15 cooperative-cancel-every-frame).

### Representative RED test bodies (real `#[test] fn`, the `model.rs` idiom: typed errors not panics, `NopStage` doubles)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // A minimal stage double, the NopLoader analog: records inputs, programmable route/wait.
    #[derive(Default)]
    struct NopStage {
        id: StageId,
        outputs_to: Vec<StageId>,
        reset_calls: std::cell::RefCell<Vec<SlotId>>,
    }
    impl SlotResettable for NopStage {
        fn reset_slot(&mut self, slot: SlotId) { self.reset_calls.get_mut().push(slot); }
    }

    // ---- (b).1 route_fn ----
    #[test]
    fn route_fn_off_topology_target_is_bad_config_not_silent_drop() {
        // outputs_to declares only stage 2; route_fn returns stage 9 (off-topology).
        let node = StageNode::test_node(StageId(1), vec![StageId(2)])
            .with_route(|_ctx, _data| RouteDecision::To(StageId(9)));
        let err = node.route(&RequestCtx::default(), &Payload::empty()).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadConfig);            // typed error, NOT a panic, NOT silent
        assert!(err.message.contains("off-topology"), "got: {}", err.message);
    }

    #[test]
    fn route_fn_drop_routes_to_terminal_sink_never_deadlocks_a_join() {
        // A VAD-silent frame must Drop to the sink, NOT to `[]` (which would hang a waiting join).
        let node = StageNode::test_node(StageId(1), vec![StageId(2)])
            .with_route(|_ctx, data| if data.is_silence() { RouteDecision::Drop }
                                     else { RouteDecision::To(StageId(2)) });
        assert_eq!(node.route(&RequestCtx::default(), &Payload::silence()).unwrap(),
                   RouteDecision::Drop);
    }

    // ---- (b).2 wait_for_fn (the G11 deadlock fix) ----
    #[test]
    fn text_only_request_join_releases_without_audio_branch_no_deadlock() {
        // Static inputs = {stt_text, audio_enc}; a text-only request never produces audio_enc.
        // The join must release on the per-request expected set {stt_text}, NOT the static superset.
        let mut join = JoinNode::new(JoinPolicy::OnExpectedSet,
            /*wait_for_fn*/ |ctx| if ctx.is_text_only { ExpectedSources(smallvec![StageId(10)]) }
                                  else { ExpectedSources(smallvec![StageId(10), StageId(11)]) });
        let ctx = RequestCtx { is_text_only: true, ..Default::default() };
        join.accept(SlotId(0), &ctx, source_frame(StageId(10)));
        assert!(join.ready(SlotId(0)), "text-only join must release on {{stt_text}} alone");
    }

    // ---- (b).3 multi-terminal per-terminal FINAL ----
    #[test]
    fn final_on_transcript_terminal_does_not_close_audio_terminal() {
        let mut dag = MultiTerminalDag::two_terminal(TerminalId(0)/*text*/, TerminalId(1)/*audio*/);
        dag.emit_final(SlotId(0), TerminalId(0));
        assert!(dag.terminal_open(SlotId(0), TerminalId(1)),
                "audio terminal must stay open after the transcript FINAL");
        assert!(!dag.request_complete(SlotId(0)), "request completes only when ALL terminals FINAL");
    }

    #[test]
    fn request_narrows_terminals_text_only_drops_audio_terminal() {
        let node = StageNode::fanout(Terminals(smallvec![TerminalId(0), TerminalId(1)]));
        let ctx = RequestCtx { requested_terminals: vec![TerminalId(0)], ..Default::default() };
        assert_eq!(node.effective_terminals(&ctx), vec![TerminalId(0)]);
    }

    // ---- (b).4 FINAL after tail drain + cancelled≠completed ----
    #[test]
    fn stage_holds_final_until_its_delay_tail_drains_then_forwards_after_tail_drain_true() {
        let mut egress = StageEgress::with_delay_tail(StageId(3), /*tail_frames*/ 4);
        // Tail not drained yet → FINAL is HELD (None), not forwarded (else next stage truncates).
        let up = EdgeSignal::Final { from_stage: StageId(2), channel: ChannelId(7),
                                     after_tail_drain: true };
        assert!(egress.on_upstream_final(up).is_none(), "FINAL must be held until tail drains");
        for _ in 0..4 { egress.step_tail(ChannelId(7)); }     // drain the 4 buffered frames
        match egress.poll_drain(ChannelId(7)) {
            Some(EdgeSignal::Final { from_stage, after_tail_drain, .. }) => {
                assert_eq!(from_stage, StageId(3));
                assert!(after_tail_drain, "forwarded FINAL must assert its own tail drained");
            }
            other => panic!("expected forwarded FINAL after drain, got {other:?}"),
        }
    }

    #[test]
    fn cancelled_propagates_through_every_stage_distinct_from_final() {
        let mut dag = LinearDag::n_stages(5);
        dag.barge_in(SlotId(0), ChannelId(1));
        let term = dag.drive_to_terminal(SlotId(0));
        // The client sees CANCELLED, never FINAL (catalog G2: cancelled ≠ completed end-to-end).
        assert!(matches!(term, TerminalFrame::Cancelled { .. }),
                "barge-in must yield Cancelled, not Final, through all 5 stages");
    }

    // ---- (b).5 SentenceAggregator ----
    #[test]
    fn aggregator_commits_only_on_sentence_boundary_emits_each_span_once_on2_free() {
        let mut agg = StableSpanGate::new(BoundaryRule::SentenceTerminator, /*stable_after*/ 3);
        assert!(agg.push("Hello wor").is_empty());            // no boundary yet
        assert!(agg.push("ld how are").is_empty());           // still no terminator
        let spans = agg.push("you? And mo");                  // "?" closes a span
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "Hello world how are you?");
        // The committed span is NEVER re-emitted (the O(N) guarantee, not O(N²)).
        let next = agg.push("re text.");
        assert_eq!(next[0].text, " And more text.");          // only the NEW span, no re-translate
    }

    #[test]
    fn aggregator_flush_emits_trailing_uncommitted_clause_on_final() {
        let mut agg = StableSpanGate::new(BoundaryRule::SentenceTerminator, 3);
        agg.push("a tail with no period");                    // no terminator → uncommitted
        let last = agg.flush().expect("flush must emit the trailing clause, not drop it");
        assert_eq!(last.text, "a tail with no period");
    }

    // ---- (b).6 DagSlotReset + DAG-wide channel_id ----
    #[test]
    fn dag_slot_reset_fans_to_every_stage_and_bumps_channel_id() {
        let mut stages: Vec<Box<dyn SlotResettable>> =
            vec![Box::new(NopStage::default()), Box::new(NopStage::default()),
                 Box::new(NopStage::default())];
        let mut channels = vec![ChannelId(0); 4];
        let before = channels[0];
        DagSlotReset { stages: &mut stages, channel_ids: &mut channels }.reset(SlotId(0));
        assert!(channels[0] > before, "reset must bump the DAG-wide ChannelId (stale-drop guard)");
        // EVERY stage saw reset_slot(0) — the FEAT-55 fan-out (not just the AR model).
        // (downcast in the real test; asserted here via a recording double per stage)
    }

    #[test]
    fn stale_channel_frame_from_prev_occupant_dropped_at_every_stage() {
        let live = ChannelId(5);
        let stale = EdgeSignal::Data { payload: Payload::empty(), channel: ChannelId(4),
                                       frame_idx: 99 };
        assert!(!stage_accepts(&stale, live), "a prev-occupant frame (id<live) must be dropped");
        let fresh = EdgeSignal::Data { payload: Payload::empty(), channel: ChannelId(5),
                                       frame_idx: 0 };
        assert!(stage_accepts(&fresh, live), "the live occupant's frame must pass");
    }

    // ---- (b).7 CloudStage ----
    #[test]
    fn cloud_stage_disconnect_fans_error_to_dag_and_fails_request_not_hangs() {
        let mut cloud = FakeCloudStage::that_disconnects_after(2);
        cloud.open(SlotId(0), &RequestCtx::default()).unwrap();
        cloud.relay(SlotId(0), data_frame(0)).unwrap();
        let out = cloud.relay(SlotId(0), data_frame(1)).unwrap();   // disconnect happens here
        assert!(out.iter().any(|s| matches!(s, EdgeSignal::Error { err, .. }
                                            if err.code == ErrorCode::StallTimeout)),
                "cloud disconnect must emit Error (fail-fast), not silently stall the DAG");
    }

    #[test]
    fn barge_in_invokes_cloud_remote_cancel_and_awaits_its_ack() {
        let cloud = FakeCloudStage::default();
        let acked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        // cancel_remote must be called AND must ACK the DAG-wide barge-in.
        cloud.on_cancel_remote({ let a = acked.clone(); move || a.store(true, SeqCst) });
        BargeIn::with(&[cloud.handle()]).cancel_all(SlotId(0), ChannelId(1), Duration::from_millis(50))
            .expect("cloud must ACK barge-in within deadline");
        assert!(acked.load(SeqCst), "barge-in must reliably cancel the remote cloud session");
    }

    // ---- (b).8 reliable ACK'd fan-out ----
    #[test]
    fn barge_in_fans_to_all_stages_and_errors_naming_a_stage_that_does_not_ack() {
        let good = StageHandle::always_acks();
        let stuck = StageHandle::never_acks();                 // a dropped/late stage (the G9 hazard)
        let err = BargeIn::with(&[good, stuck])
            .cancel_all(SlotId(0), ChannelId(1), Duration::from_millis(10))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::StallTimeout);
        assert!(err.message.contains("did not ACK"), "must name the non-ACKing stage (force-free)");
    }
}
```

**Type count (new public types in `dag/`):** `SlotId`, `StageId`, `TerminalId`, `ChannelId`, `RouteFn`/`RouteDecision`, `WaitForFn`/`ExpectedSources`, `Terminals`, `DagFields`, `JoinPolicy`, `EdgeSignal`, `TerminalFrame`, `CancelReason`, `StageEgress`(+`FinalGate`), `SpanAggregator`(trait)/`StableSpanGate`/`CommittedSpan`/`BoundaryRule`, `SlotResettable`(trait), `DagSlotReset`, `CloudStage`(trait)/`RemoteSession`/`NetworkSlo`, `BargeIn`/`Cancel`/`CancelAck`. **= 22 types/traits.**

**Test count (RED gates):** the 13 representative bodies above map 1:1 to the v2.1 §6.2 / M4.1b gate names plus 3 the audit asked for (`request_narrows_terminals`, `aggregator_flush_*`, `barge_in_storm` reuses the ACK path). **= 13 representative RED tests** (the full M4.1b matrix is the 10 §6.2 gates + these refinements).

---

## (c) Residual gaps

After this design, two scenario twists are **NOT** fully closed by L2 alone — both because the missing piece lives in a *different* layer; L2 supplies the DAG machinery but not the upstream signal. Plus three adversarial NEW interaction findings.

### Residual 1 — Per-span code-switch re-routing (FEAT-60, S2S-66) — needs L1 langID + L3 active-span
**Why L2 is insufficient.** `route_fn` is evaluated per-frame, so re-routing *is* mechanically possible per span. But the *trigger* — a per-span frame-level language label — is an **L1 langID feature-stage** output, and "re-segment interleaved multilingual spans into coherent target sentences" needs the `SpanAggregator` (L2, ✓) to be **language-aware** (commit a span when the language changes, not only on punctuation) and the MT node to consume per-language committed spans. **L2 closes the routing + aggregation machinery; the per-span langID signal and the language-change boundary rule are the residual.** *Seam:* add `BoundaryRule::OnLanguageChange` (consumes an L1 langID side-edge) — a 1-variant addition to `StableSpanGate`, but it depends on L1 shipping the langID stage. **Carry to L1+L2 joint gate** `per_span_langid_reroutes_within_stream` (already named M5c).

### Residual 2 — Time-aligned join where a silent speaker must not stall (FEAT-18/41/65, S2S-48) — needs L3 active-set
**Why L2 is insufficient.** `JoinByTime` (§(b).3) aligns two branches by frame-stamp and bounds memory, closing the *temporal merge*. But "a silent speaker's branch produces no frames, and the merge must NOT stall waiting for it" requires knowing the **per-frame active-speaker set** — which is the **L3 duplex active-set / per-stream role** signal, not a DAG-topology fact. `wait_for_fn` is per-request (fixed for the request), not per-frame; a silent speaker is a per-frame condition. **L2 closes the join; the per-frame active-set that lets the join advance past a silent branch is the residual.** *Seam:* `JoinPolicy::ByTime` needs an `active_set_fn(frame_idx) -> set` hook fed by L3. **Carry to L2+L3 joint gate** `silent_speaker_does_not_stall_merge`.

### Residual 3 [NEW — adversarial] — FINAL-after-tail-drain vs the streaming-window archetype's first-chunk RAMP
**The interaction the prompt asked about.** `FinalGate` (§(b).4) holds FINAL until a stage's delay tail drains. But the **streaming-vocoder archetype** (`INFER_ENGINE.md` §3.2: left-context + crossfade + *dynamic first-chunk TTFA ramp*) has a tail that is **not a fixed frame count** — the last chunk may be a *short* partial window that the crossfade logic pads. `tail_drained(channel)` as written assumes a countable tail (F5 marker / F8 ring depth). For a streaming-window stage, "drained" means "the last partial window has been emitted AND its crossfade tail has decayed" — a **per-archetype `tail_drained` predicate**, not a universal frame-count. **NEW residual:** `FinalGate::tail_drained` must dispatch on the stage's archetype (`Ar` → marker-heap empty; `nested` → inner-solver flushed; `streaming_window` → last-window-crossfade-complete). This is a real hole: a naive frame-count tail predicate would forward FINAL one crossfade-tail early on a vocoder stage → an audible truncated final chunk. *Fix:* make `tail_drained` a method on the stage's archetype trait, not a field. **Add gate** `final_gate_tail_predicate_is_per_archetype` (streaming-window crossfade-complete ≠ AR marker-empty).

### Residual 4 [NEW — adversarial] — `DagSlotReset` composition order vs in-flight inner-solver latent
**The composition the prompt asked about.** `DagSlotReset` (§(b).6) fans `reset_slot` to every stage including an AR-outer/generative-inner node (R2 third class). That node has TWO reset participants: `ArStepModel::reset_slot` (outer KV/RNG) AND the **inner-solver latent** (the nested CFM/ODE state, IMPL M4.2 `triple_nested_reset_slot_fans_out_to_inner_solver_state`). If `DagSlotReset` calls only the outer `reset_slot`, a recycled slot inherits the *previous occupant's half-finished inner latent* → cross-user contamination INSIDE the nested forward (a place F3 never looked). **NEW residual:** `SlotResettable` for a nested node must reset BOTH the outer AND the inner latent **atomically** — and the ordering matters (reset inner before outer, else an in-flight inner step writes back into a just-cleared outer slot, the catalog G8 "stale-batch overrun re-runs a finished req" landmine). L2's `DagSlotReset` correctly bumps `ChannelId` first (which makes the in-flight inner output stale-droppable), but the *intra-node* two-part reset is an M4.2 contract L2 must REQUIRE, not assume. *Fix:* `SlotResettable::reset_slot` for a nested node is documented to reset inner-then-outer; L2's transaction depends on it. **Add gate** `nested_stage_reset_slot_clears_inner_latent_before_outer`.

### Residual 5 [NEW — adversarial] — `route_fn` static-topology constraint genuinely blocks one real case
**The constraint the prompt probed.** `route_fn` must return ∈ static `outputs_to` (so VRAM/topology stays analyzable — ties J2). This is correct for branch *selection* among pre-loaded candidates. But **per-span code-switch** (Residual 1) wants to route to a language-specific STT that may **not be resident** (N idle language models would blow VRAM — the FEAT-7 `If mishandled` the audit flagged). The static-topology rule means *all* candidate STT models must be in the static residency set — which collides with the VRAM accountant for many languages. **This is NOT an L2 bug — it is the correct, conservative behavior** (better to refuse an unanalyzable topology than OOM mid-call). But it means per-span N-language routing is gated on an **L4/L5 mechanism**: either (a) a LoRA-adapter-per-language over one base (J14, swap=ms, fits the static-topology rule because it's ONE node with hot-swappable weights), or (b) admission-time language-set declaration (the request declares its ≤k candidate languages, only those k are made resident). **L2's constraint stands; the residual is that the *scalable* per-span multilingual case needs the L5 LoRA seam.** *No new L2 gate* — this is a documented constraint boundary, resolved in L4/L5. (Recorded so the constraint is not mistaken for a defect.)

---

## Verdict

**9 of 11 prior-PARTIAL/GAP DAG scenarios CLOSED** by the deep design (the 9 in the §(a) table marked CLOSED — every v2.1 §6.2 / M4.1b gate-name now has a real type + algorithm + RED test); **2 scenario twists RESIDUAL** because they require a cross-layer signal (L1 langID per-span, L3 active-set) that L2 cannot manufacture — L2 supplies the routing/join machinery, the upstream signal is the missing half. **22 types/traits, 13 representative RED tests** (mapping 1:1 to the M4.1b gate matrix + 3 audit refinements). **5 residual gaps total:** 2 cross-layer (Res-1/2, seams named), 3 NEW adversarial interaction findings (Res-3 FINAL×streaming-window crossfade-tail predicate; Res-4 DagSlotReset×nested inner-latent ordering; Res-5 route_fn static-topology×multilingual residency — a documented constraint boundary, not a defect). Each of Res-3/4 yields a new named gate (`final_gate_tail_predicate_is_per_archetype`, `nested_stage_reset_slot_clears_inner_latent_before_outer`) to add to M4.1b/M4.2.
