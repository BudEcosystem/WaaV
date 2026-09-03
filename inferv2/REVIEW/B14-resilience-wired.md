# B14 — Wiring the resilience-layer shelfware into the LIVE poller

**Status: DONE.** Tests + clippy green for the two target crates. The dormant defense legs the review flagged as built-but-never-polled (`01-runtime.md` HIGH-1, `05-crosscutting-landmines.md` C1 + H1 + H3) are now driven by the live out-of-band watchdog poller. Bit-faithful: no accepted-stream numerics touched (recovery + observability only); the over-defended watchdog arithmetic is untouched.

Files changed (ONLY these — the allowed set):
- `crates/waav-infer-runtime/src/watchdog.rs` — new wiring types (`SidecarRegistry`, `Quarantine`) + `RecycleGate` bounded eviction + process-global registry singleton + 6 tests.
- `crates/waav-infer-runtime/src/lib.rs` — re-export the new types (mechanical, to surface them).
- `crates/waav-infer-server/src/lib.rs` — `ProdSpine` carries the two legs; `spawn_watchdog` drives them; `AppState` exposes the crash-report / admit / clear hooks.
- `crates/waav-infer-server/src/engine.rs` — the sidecar-construction seam enrolls each spawned sidecar's heartbeat in the registry.

> Note: `git status` also shows `codec_ar_admission.rs` and the whole `waav-infer-scheduler` crate as modified — those are **other concurrent efforts' uncommitted work** (457 / 333 insertions, timestamped today, "Not Committed Yet"), NOT mine. I never opened them.

---

## Which legs are now live-polled

| Leg | Was | Now (live poller drives it) | Where |
|---|---|---|---|
| **Sidecar idle-zombie scan** (C1, the cross-cutting [CRIT]) | `SidecarHeartbeat::check_at` had ZERO production poller — an idle-wedged-between-requests sidecar was tracked but never reaped | `spawn_watchdog` loop calls `spine.sidecar_registry.scan_and_reap(now)` every 1 s → latches the dead-flag on any sidecar silent past its heartbeat window → its next `authorize_serve` fails typed (the dead-flag fan-out) instead of a session hanging on its pipe | poll: `server/lib.rs::spawn_watchdog` leg 4; type: `watchdog.rs::SidecarRegistry`; registration: `engine.rs::load_torch_model` → `register_sidecar(sidecar.watchdog())` |
| **Poison-pill firewall** (H1) | `InputFirewall` built+exported, zero callers | bundled into `Quarantine`; `report_decode_crash` records the per-`(channel,signature)` crash and trips it on the 2nd identical crash | `watchdog.rs::Quarantine::{report_decode_crash, admit}` |
| **Dead-letter** (H1) | `DeadLetterSink` built+exported, zero callers | on a tripped (quarantined) verdict, `report_decode_crash` `capture`s the input into the sink (the held, enumerable destination) | `watchdog.rs::Quarantine::report_decode_crash` |
| **Crash-loop quarantine / source rate-limit** (H1) | `SourceRateLimiter` built+exported, zero callers | each confirmed dead-letter charges the source's restart-rate; a source flooding distinct pills past budget is throttled (retriable 429) at `admit` | `watchdog.rs::Quarantine::{report_decode_crash, admit}` |
| **Quarantine eviction** (H3) | the firewall/sink/limiter maps had only test-only `clear_*` → a leak the instant they were wired | `spawn_watchdog` loop calls `spine.quarantine.evict_expired(now, TTL=10min)` every 1 s → ages out a dead-lettered channel (clears all three legs + the side-stamp) so the maps stay bounded under churn | poll: `server/lib.rs::spawn_watchdog` leg 5; logic: `watchdog.rs::Quarantine::evict_expired` |
| **`RecycleGate.states` bounded eviction** (H3 — the "wire it and you get a leak" landmine) | NO eviction at all (not even a test-only clear) — insert-and-mutate-only, `ChannelId` minted fresh per stream | added `clear_channel(channel)` (the F3 per-slot clear, mirrors siblings) **and** a hard cap (`MAX_STATES = 4096`, >> `MAX_SLOTS=24`): `request_recycle` evicts the oldest *settled* entries past the cap (LRU-by-age via ascending id order), **never a pending recycle** | `watchdog.rs::RecycleGate::{clear_channel, MAX_STATES, evict_settled_over_cap}` |

### How the construction/poll seams rendezvous (substrate-agnostic, control-plane)

A torch sidecar is constructed deep in the model-load path (`engine.rs::load_torch_model`) **before** the `ProdSpine` that the poller carries exists, and its `SidecarHeartbeat` is then buried inside an opaque `dyn TtsModel`/`dyn SttModel`. So the registration point and the poll point cannot pass a handle hand-to-hand. They rendezvous on a **process-global `OnceLock` singleton** (`process_sidecar_registry()` / `register_sidecar()` — the same idiom as the existing `process_monotonic_now()`). `ProdSpine::new` clones the singleton into `spine.sidecar_registry`, so the poller scans the same registry the construction seam enrolls into. `watchdog()` already existed as a `pub` accessor on `TorchSidecar`, so **torch_sidecar.rs was NOT touched** (constraint honored). The `Quarantine` is a normal per-spine field (its hooks are reached via `AppState`, owned by the server crate I may touch).

---

## Tests (all in `watchdog.rs`, all green)

Added 6:
1. `idle_sidecar_scan_trips_a_wedged_idle_sidecar` — a registered sidecar that beats once then goes silent is declared dead by `scan_and_reap` once past its 15 s window; the latch lands on the shared ledger so `authorize_serve` then fails typed; a still-beating sidecar is NOT reaped (no false trip); the report tally is exact.
2. `sidecar_scan_empty_and_healthy_are_clean_noops` — the ORT-only deployment (empty registry) and a steady-state beating fleet are clean no-ops (the poller doesn't disturb them).
3. `crash_loop_input_is_quarantined_and_admission_refuses_replay` — 1st crash absorbed (replay budget), 2nd identical crash dead-letters → `admit` of that exact input refused (typed 4xx `BadConfig`) while a different input on the same channel still admits; re-report is idempotent (no count run-away).
4. `flooding_source_is_rate_limited_after_distinct_poison_pills` — a source minting 3 distinct dead-lettering pills (budget 2) is throttled (retriable 429 `AdmissionRejected`); a well-behaved source is untouched.
5. `quarantine_clear_and_ttl_eviction_keep_maps_bounded` — the per-slot `clear_channel` drops a quarantine (recycled id starts fresh); the poller's `evict_expired` reclaims an aged-out one (maps bounded); a stamp ahead of `now` (clock skew) is NOT evicted early.
6. `recycle_gate_states_map_stays_bounded_under_churn` + `recycle_gate_eviction_never_drops_a_pending_recycle` — pushing `MAX_STATES + 500` settled recycles keeps the ledger at exactly `MAX_STATES` (oldest evicted, newest survive, `clear_channel` drops one); and under an in-flight kernel where every request is *pending*, NOT ONE pending recycle is evicted and all drain at the boundary (the J23 witness stays 0).

Existing watchdog/recycle/sidecar tests (the over-defended arithmetic, `mid_tick_inflight_recycle_deferred`, the bit-identity gates, the per-slot isolation gates) all still pass — no regression.

**Counts:**
- `cargo test -p waav-infer-runtime --lib` → **232 passed; 0 failed**.
- `cargo test -p waav-infer-runtime -p waav-infer-server --lib` → runtime 232 + server 53 = **285 passed; 0 failed** (4 server ignored = pre-existing live-GPU-gated, unrelated).

## Clippy

- `cargo clippy -p waav-infer-runtime -- -D warnings` → **clean**.
- `cargo clippy -p waav-infer-server -- -D warnings` → **clean for the server's own code**.
- The combined `cargo clippy -p waav-infer-runtime -p waav-infer-server -- -D warnings` reports **ONE error — `waav-infer-scheduler/src/admission.rs:833` `collapsible_if`** — which is in **another effort's uncommitted code** (the offending `if !self.compute.contains_key(&s) { if best.is_none_or(...)` was added today by the scheduler effort; `git blame` shows "Not Committed Yet"). The scheduler is a forbidden file (another effort owns it). With that single pre-existing lint isolated (`-A clippy::collapsible_if`, no file change), the full combined check passes and **zero warnings reference my files** (`watchdog.rs`, `server/lib.rs`, `server/engine.rs`). My code is clippy-clean.

---

## The one leg that genuinely needs a serve-loop hook another effort owns (flagged precisely)

**The poison-pill quarantine's two CALL-SITES live in the codec-AR serve/admission path, which I was forbidden to touch** (`serve.rs`, `codec_ar_batcher.rs`, `codec_ar_admission.rs`). I built and made-reachable the stable hooks; the actual invocations must be added by the effort that owns those files:

1. **Report a decode crash** → call `AppState::report_decode_crash(channel, signature)` (delegates to `Quarantine::report_decode_crash`) at the point the serve loop catches an input that killed decode. Today nothing reports crashes, so nothing is quarantined *yet* — the machinery is wired and tested, but it fires only once the serve loop calls the hook. The `(channel, signature)` it needs are the per-stream `ChannelId` (already minted by the batcher) + the §13.6 ingress content fingerprint as the `InputSignature` (the firewall treats it as opaque).
2. **Admit-check before dispatch** → call `AppState::admit_input(channel, signature)` (delegates to `Quarantine::admit`) before dispatching an input to decode, to refuse a dead-lettered crash-loop input / throttle a flooding source. (This is the inverse of #1 — together they close the loop.)
3. **Per-slot clear on recycle** → call `AppState::clear_channel_quarantine(channel)` (and `RecycleGate::clear_channel`) on F3 slot-recycle so a recycled id starts fresh. The poller's TTL eviction is the time-based backstop if this is missed, so the H3 leak is closed regardless — but the prompt per-slot clear is the clean path the serve-loop effort should add.

**The sidecar-scan leg needs NO serve-loop hook** — it is fully live end-to-end: registration is at the construction seam (`engine.rs`, mine), the scan is in the poller (`spawn_watchdog`, mine), and the dead-flag fan-out is already consulted by the existing `TorchSidecar::request` → `authorize_serve` gate. An idle-wedged sidecar is now reaped on the 1 s cadence with no further wiring. **The RecycleGate-eviction leg also needs no serve-loop hook** for the *bound* — the hard cap in `request_recycle` is self-driving (a settled-tail eviction on every request); only the optional prompt `clear_channel` on recycle is a (covered-by-the-cap) serve-loop nicety.

### Metrics published by the poller (new)
`waav_infer_dead_sidecars` (gauge), `waav_infer_quarantine_evicted_total` (counter) — alongside the existing `waav_infer_frame_watchdog_shed_total` / `waav_infer_leaked_channels`.
