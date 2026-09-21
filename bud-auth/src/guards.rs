//! Guards for the auth MISS path.
//!
//! A hit is a hash-map probe that returns in nanoseconds. A miss becomes a Redis round trip —
//! on a path that runs **before authentication**, is reachable by anyone who can set an
//! `Authorization` header, and sits **outside the rate limiter** (auth is the outermost layer).
//! These guards are what make that safe.
//!
//! In order of how much they actually bound the damage:
//!
//! 1. **Shape pre-filter** — only two token shapes exist. Anything else costs a prefix check.
//! 2. **Global concurrency cap** — the real load bound. A *per-key* limit cannot bound an
//!    attacker who rotates keys, because the key space is attacker-controlled.
//! 3. **Circuit breaker** — sustained backend failure stops escalation entirely.
//! 4. **Negative cache** — bounds *repeats* of one key (a stale SDK, a retry loop). It is NOT
//!    the load bound, for the reason in (2).

use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// What a presented bearer could plausibly be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyShape {
    /// A `bud_*` / `budserve_*` static credential.
    Static,
    /// A compact JWS. Verified locally; never escalated to a backend.
    Jwt,
    /// Neither. Must never cost more than this check.
    Unrecognized,
}

impl KeyShape {
    pub fn classify(raw: &str) -> Self {
        if raw.starts_with("bud_") || raw.starts_with("budserve_") {
            return Self::Static;
        }
        // A compact JWS is exactly three non-empty base64url segments. A cheap structural test
        // only — it asserts nothing about validity, just that looking further is not obviously
        // pointless.
        let mut parts = raw.split('.');
        let three_segments = matches!(
            (parts.next(), parts.next(), parts.next(), parts.next()),
            (Some(a), Some(b), Some(c), None) if !a.is_empty() && !b.is_empty() && !c.is_empty()
        );
        if three_segments && raw.starts_with("ey") {
            return Self::Jwt;
        }
        Self::Unrecognized
    }

    /// May this shape reach Redis at all?
    ///
    /// Both real shapes may: a `bud_*` credential and an ephemeral bridge token both live under
    /// `api_key:{hash}`. Only unrecognised junk is refused outright.
    pub fn may_escalate(self) -> bool {
        !matches!(self, Self::Unrecognized)
    }
}

/// Why an escalation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// Not a plausible key shape — rejected with no lookup.
    Shape,
    /// Authoritatively proven absent recently.
    NegativeCache,
    /// Global in-flight cap reached.
    Overloaded,
    /// Backend is failing; stop asking.
    CircuitOpen,
}

impl Denied {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shape => "shape",
            Self::NegativeCache => "negative_cache",
            Self::Overloaded => "overloaded",
            Self::CircuitOpen => "circuit_open",
        }
    }
}

/// Permission to escalate. Releases the concurrency slot on drop.
#[derive(Debug)]
pub struct EscalationPermit {
    _permit: OwnedSemaphorePermit,
}

#[derive(Debug, Clone)]
pub struct MissGuardConfig {
    pub max_inflight: usize,
    pub negative_ttl: Duration,
    pub negative_capacity: usize,
    pub circuit_threshold: u32,
    pub circuit_cooldown: Duration,
}

impl Default for MissGuardConfig {
    fn default() -> Self {
        Self {
            max_inflight: 16,
            // Minutes-scale is safe ONLY because entries are invalidated on a `set` event for
            // that key (see `forget`). Without that invalidation no TTL is short enough: a
            // repaired key would stay denied for the remainder of its entry — the fix lands and
            // the outage continues.
            negative_ttl: Duration::from_secs(300),
            negative_capacity: 10_000,
            circuit_threshold: 5,
            circuit_cooldown: Duration::from_secs(30),
        }
    }
}

struct CircuitBreaker {
    consecutive_failures: AtomicU32,
    /// Millis since `origin` until which the circuit stays open. 0 = closed.
    open_until_ms: AtomicU64,
    origin: Instant,
    threshold: u32,
    cooldown: Duration,
}

impl CircuitBreaker {
    fn new(threshold: u32, cooldown: Duration) -> Self {
        Self {
            consecutive_failures: AtomicU32::new(0),
            open_until_ms: AtomicU64::new(0),
            origin: Instant::now(),
            threshold,
            cooldown,
        }
    }

    fn now_ms(&self) -> u64 {
        self.origin.elapsed().as_millis() as u64
    }

    fn is_open(&self) -> bool {
        let until = self.open_until_ms.load(Ordering::Relaxed);
        until != 0 && self.now_ms() < until
    }

    fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
        self.open_until_ms.store(0, Ordering::Relaxed);
    }

    fn record_failure(&self) {
        let n = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        if n >= self.threshold {
            self.open_until_ms.store(
                self.now_ms() + self.cooldown.as_millis() as u64,
                Ordering::Relaxed,
            );
        }
    }
}

pub struct MissGuards {
    /// hashed key -> when it was proven absent.
    negative: DashMap<String, Instant>,
    inflight: Arc<Semaphore>,
    circuit: CircuitBreaker,
    cfg: MissGuardConfig,
}

impl Default for MissGuards {
    fn default() -> Self {
        Self::new(MissGuardConfig::default())
    }
}

impl MissGuards {
    pub fn new(cfg: MissGuardConfig) -> Self {
        Self {
            negative: DashMap::new(),
            inflight: Arc::new(Semaphore::new(cfg.max_inflight)),
            circuit: CircuitBreaker::new(cfg.circuit_threshold, cfg.circuit_cooldown),
            cfg,
        }
    }

    /// Decide whether `raw_key` may cost a backend round trip.
    ///
    /// Order matters: the cheapest, most-bounding checks run first, so junk never reaches the
    /// semaphore and a known-absent key never consumes a slot.
    pub fn try_escalate(
        &self,
        raw_key: &str,
        hashed_key: &str,
    ) -> Result<EscalationPermit, Denied> {
        if !KeyShape::classify(raw_key).may_escalate() {
            return Err(Denied::Shape);
        }
        if self.is_known_absent(hashed_key) {
            return Err(Denied::NegativeCache);
        }
        if self.circuit.is_open() {
            return Err(Denied::CircuitOpen);
        }
        match Arc::clone(&self.inflight).try_acquire_owned() {
            Ok(permit) => Ok(EscalationPermit { _permit: permit }),
            Err(_) => Err(Denied::Overloaded),
        }
    }

    fn is_known_absent(&self, hashed_key: &str) -> bool {
        match self.negative.get(hashed_key).map(|e| *e.value()) {
            Some(at) if at.elapsed() < self.cfg.negative_ttl => true,
            Some(_) => {
                // Expired: drop it so the map does not accumulate dead entries.
                self.negative.remove(hashed_key);
                false
            }
            None => false,
        }
    }

    /// Record that the backend authoritatively reported this key absent.
    ///
    /// Only for an AUTHORITATIVE absence. A transient failure must never land here — unavailable
    /// is not absent, and caching it would turn a wobble into `negative_ttl` of denial.
    pub fn record_absent(&self, hashed_key: &str) {
        if self.negative.len() >= self.cfg.negative_capacity {
            // Arbitrary eviction is fine; the goal is a bound, not an LRU. Taking one key and
            // dropping the iterator before removing avoids holding a shard guard across the
            // mutation, which DashMap can deadlock on.
            let victim = self.negative.iter().next().map(|e| e.key().clone());
            if let Some(v) = victim {
                self.negative.remove(&v);
            }
        }
        self.negative.insert(hashed_key.to_string(), Instant::now());
    }

    /// Drop any negative entry for a key.
    ///
    /// Wired to the Redis `set` keyspace event. Without it, a key repaired in Redis — created,
    /// re-created, expiry extended, or restored by a resync — stays denied for the rest of its
    /// negative TTL: the fix lands and the outage continues.
    pub fn forget(&self, hashed_key: &str) {
        self.negative.remove(hashed_key);
    }

    pub fn record_backend_success(&self) {
        self.circuit.record_success();
    }

    pub fn record_backend_failure(&self) {
        self.circuit.record_failure();
    }

    pub fn negative_len(&self) -> usize {
        self.negative.len()
    }

    pub fn available_slots(&self) -> usize {
        self.inflight.available_permits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JWT: &str = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ1In0.c2ln";

    #[test]
    fn classifies_the_three_shapes() {
        assert_eq!(KeyShape::classify("bud_abc"), KeyShape::Static);
        assert_eq!(KeyShape::classify("budserve_abc"), KeyShape::Static);
        assert_eq!(KeyShape::classify(JWT), KeyShape::Jwt);
        assert_eq!(KeyShape::classify("garbage"), KeyShape::Unrecognized);
        assert_eq!(KeyShape::classify(""), KeyShape::Unrecognized);
    }

    #[test]
    fn near_miss_jwts_are_not_jwts() {
        // Three segments but not base64url-ish start.
        assert_eq!(KeyShape::classify("aaa.bbb.ccc"), KeyShape::Unrecognized);
        // Empty middle segment.
        assert_eq!(KeyShape::classify("ey..c2ln"), KeyShape::Unrecognized);
        // Four segments (JWE, not JWS).
        assert_eq!(KeyShape::classify("ey.a.b.c"), KeyShape::Unrecognized);
        // Two segments.
        assert_eq!(KeyShape::classify("ey.a"), KeyShape::Unrecognized);
    }

    /// TC-GUARD-01 — junk must cost a prefix compare and nothing more.
    #[test]
    fn junk_is_refused_on_shape_before_any_slot_is_taken() {
        let g = MissGuards::default();
        let before = g.available_slots();

        for junk in ["garbage", "", &"x".repeat(1_000_000)] {
            assert_eq!(g.try_escalate(junk, "h").unwrap_err(), Denied::Shape);
        }
        assert_eq!(
            g.available_slots(),
            before,
            "junk consumed a concurrency slot; the shape filter is not running first"
        );
    }

    /// TC-GUARD-02 — the global cap is the real load bound.
    ///
    /// Distinct keys on purpose: a per-key limit would pass this test and still fall over,
    /// because the key space is attacker-controlled.
    #[test]
    fn global_cap_bounds_concurrent_misses_across_distinct_keys() {
        let cfg = MissGuardConfig {
            max_inflight: 16,
            ..Default::default()
        };
        let g = MissGuards::new(cfg);

        let mut held = Vec::new();
        for i in 0..1000 {
            if let Ok(p) = g.try_escalate("bud_x", &format!("h{i}")) {
                held.push(p);
            }
        }
        assert_eq!(
            held.len(),
            16,
            "in-flight escalations exceeded the cap; 1000 rotated keys got {} slots",
            held.len()
        );

        drop(held);
        assert!(
            g.try_escalate("bud_x", "h-after").is_ok(),
            "slots not released on drop"
        );
    }

    /// TC-GUARD-03 / TC-GUARD-04
    #[test]
    fn circuit_opens_after_the_threshold_and_closes_after_cooldown() {
        let g = MissGuards::new(MissGuardConfig {
            circuit_threshold: 5,
            circuit_cooldown: Duration::from_millis(60),
            ..Default::default()
        });

        for _ in 0..4 {
            g.record_backend_failure();
        }
        assert!(g.try_escalate("bud_x", "h1").is_ok(), "opened early");

        g.record_backend_failure();
        assert_eq!(
            g.try_escalate("bud_x", "h2").unwrap_err(),
            Denied::CircuitOpen
        );

        std::thread::sleep(Duration::from_millis(90));
        assert!(g.try_escalate("bud_x", "h3").is_ok(), "never closed");
    }

    #[test]
    fn a_success_resets_the_failure_run() {
        let g = MissGuards::new(MissGuardConfig {
            circuit_threshold: 3,
            ..Default::default()
        });
        g.record_backend_failure();
        g.record_backend_failure();
        g.record_backend_success();
        g.record_backend_failure();
        g.record_backend_failure();
        assert!(
            g.try_escalate("bud_x", "h").is_ok(),
            "failures accumulated across a success"
        );
    }

    /// TC-GUARD-05
    #[test]
    fn negative_cache_bounds_repeats_of_one_key() {
        let g = MissGuards::default();
        g.record_absent("h-missing");
        assert_eq!(
            g.try_escalate("bud_x", "h-missing").unwrap_err(),
            Denied::NegativeCache
        );
    }

    /// TC-GUARD-06 — the invalidation without which no TTL is short enough.
    ///
    /// Delete `forget()`'s call site and a key repaired in Redis stays denied for 300s.
    #[test]
    fn a_set_event_clears_the_negative_entry() {
        let g = MissGuards::default();
        g.record_absent("h1");
        assert!(g.try_escalate("bud_x", "h1").is_err());

        g.forget("h1"); // what the `set` keyspace event triggers

        assert!(
            g.try_escalate("bud_x", "h1").is_ok(),
            "repaired key still denied; the fix lands and the outage continues"
        );
    }

    #[test]
    fn negative_entries_expire_on_their_own() {
        let g = MissGuards::new(MissGuardConfig {
            negative_ttl: Duration::from_millis(40),
            ..Default::default()
        });
        g.record_absent("h1");
        assert!(g.try_escalate("bud_x", "h1").is_err());
        std::thread::sleep(Duration::from_millis(70));
        assert!(g.try_escalate("bud_x", "h1").is_ok());
    }

    /// TC-GUARD-07
    #[test]
    fn negative_cache_is_capacity_bounded() {
        let g = MissGuards::new(MissGuardConfig {
            negative_capacity: 100,
            ..Default::default()
        });
        for i in 0..20_000 {
            g.record_absent(&format!("h{i}"));
        }
        assert!(
            g.negative_len() <= 100,
            "negative cache grew to {} with a cap of 100",
            g.negative_len()
        );
    }

    /// A JWT may reach Redis (ephemeral tokens live under `api_key:{hash}` too), but must never
    /// be refused on shape.
    #[test]
    fn jwt_shape_may_escalate() {
        let g = MissGuards::default();
        assert!(g.try_escalate(JWT, "h").is_ok());
    }
}
