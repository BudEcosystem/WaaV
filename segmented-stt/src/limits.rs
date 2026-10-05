//! Every time limit of one upload, computed in one place, and the latency store that feeds it.
//!
//! `segment_deadlines` is the only producer of a time limit (integration decision 2). The engine
//! computes one thing itself, the deadline *instant*, and the transcriber layer applies the rest.
//!
//! | Limit | Counted from | Value |
//! | --- | --- | --- |
//! | Queue allowance | hand-over | 1,500 ms |
//! | Stall timeout | the first request's first byte | 1.25 × round-trip P99 + 250 ms + 50 ms per second of audio beyond 5 s; at least 1,500 ms, at most deadline − 2,500 ms |
//! | Request limit | each request's start | the row's value, at most 10,000 ms |
//! | Deadline | the turn's newest cut at hand-over | 6,000 ms; a deployment may set 3,000 to 10,000 |
//! | Room for a second request | when it would start | max(500 ms, round-trip median + 100 ms) must remain |

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::time::Instant;

pub const QUEUE_ALLOWANCE_MS: u32 = 1500;
pub const DEFAULT_DEADLINE_MS: u32 = 6000;
pub const DEADLINE_RANGE_MS: (u32, u32) = (3000, 10_000);
pub const MAX_REQUEST_LIMIT_MS: u32 = 10_000;
/// The global default when a row has no figure: end of speech to final, P99.
pub const GLOBAL_DEFAULT_P99_MS: u32 = 2000;
/// Published figures were measured with a 200 ms pause before the cut.
pub const PUBLISHED_PAUSE_MS: u32 = 200;
/// The two-and-a-half-second target of the brief (integration decision 15).
pub const SLOW_TARGET_MS: u32 = 2500;

/// The limits of one upload unit, handed whole to the transcriber layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentDeadlines {
    pub queue_allowance: Duration,
    pub stall_timeout: Duration,
    /// The early second request of the low-latency tier.
    pub hedge_delay: Option<Duration>,
    pub request_limit: Duration,
    /// What must remain before the deadline for a second request to be worth sending.
    pub second_request_room: Duration,
    /// The deadline value; the engine fixes the instant.
    pub deadline: Duration,
}

/// The raised deadline in force (Addendum A5): a deployment deadline below the silence ceiling
/// plus 2,500 ms is raised to that sum. Returns the value and whether it was raised.
pub fn effective_deadline_ms(deployment: Option<u32>, silence_ceiling_ms: u32) -> (u32, bool) {
    let asked = deployment
        .unwrap_or(DEFAULT_DEADLINE_MS)
        .clamp(DEADLINE_RANGE_MS.0, DEADLINE_RANGE_MS.1);
    let floor = silence_ceiling_ms + 2500;
    if asked < floor {
        (floor, true)
    } else {
        (asked, deployment.is_some() && deployment != Some(asked))
    }
}

/// The bound within which a turn closes, counted from its last voiced sample: the cut pause, the
/// deadline, the 250 ms guard and the 448 ms onset window (Addendum B1).
pub fn resolution_deadline_ms(cut_pause_ms: u32, deadline_ms: u32) -> u32 {
    cut_pause_ms + deadline_ms + 250 + 448
}

/// What `segment_deadlines` reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeadlineInputs {
    /// The round-trip P99 (end of speech to final less the published pause), ms.
    pub round_trip_p99_ms: u32,
    pub round_trip_p50_ms: u32,
    pub round_trip_p95_ms: u32,
    /// Real audio in the unit, ms.
    pub audio_ms: u32,
    /// The deadline in force, already raised.
    pub deadline_ms: u32,
    /// The row's limit on one request.
    pub row_request_timeout_ms: Option<u32>,
    /// The low-latency tier sends an early hedged request.
    pub low_latency: bool,
}

/// The only producer of upload time limits.
pub fn segment_deadlines(i: &DeadlineInputs) -> SegmentDeadlines {
    let beyond_5s = i.audio_ms.saturating_sub(5000) as f64 / 1000.0;
    let raw = 1.25 * i.round_trip_p99_ms as f64 + 250.0 + 50.0 * beyond_5s;
    let cap = i.deadline_ms.saturating_sub(2500).max(1500);
    let stall = (raw.round() as u32).clamp(1500, cap);
    let request = i
        .row_request_timeout_ms
        .unwrap_or(MAX_REQUEST_LIMIT_MS)
        .min(MAX_REQUEST_LIMIT_MS);
    let room = 500.max(i.round_trip_p50_ms + 100);
    SegmentDeadlines {
        queue_allowance: Duration::from_millis(QUEUE_ALLOWANCE_MS as u64),
        stall_timeout: Duration::from_millis(stall as u64),
        hedge_delay: i
            .low_latency
            .then(|| Duration::from_millis(i.round_trip_p95_ms.max(300) as u64)),
        request_limit: Duration::from_millis(request as u64),
        second_request_room: Duration::from_millis(room as u64),
        deadline: Duration::from_millis(i.deadline_ms as u64),
    }
}

/// How much evidence an estimate rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatencyBasis {
    /// A published figure, vendor claim or operator value (fewer than 30 fresh samples).
    Seed,
    Provisional,
    Measured,
    /// No figure at all.
    None,
}

impl LatencyBasis {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Seed => "seed",
            Self::Provisional => "provisional",
            Self::Measured => "measured",
            Self::None => "none",
        }
    }
}

/// The customer-facing latency class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatencyClass {
    Realtime,
    Fast,
    Slow,
    Unknown,
}

impl LatencyClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Realtime => "realtime",
            Self::Fast => "fast",
            Self::Slow => "slow",
            Self::Unknown => "unknown",
        }
    }

    /// A 99th percentile at or under 600 ms is realtime, at or under 1,200 ms fast, above slow.
    pub fn of(p99_ms: Option<u32>, realtime_max: u32, fast_max: u32) -> Self {
        match p99_ms {
            None => Self::Unknown,
            Some(v) if v <= realtime_max => Self::Realtime,
            Some(v) if v <= fast_max => Self::Fast,
            Some(_) => Self::Slow,
        }
    }
}

/// The store's estimate for one key: end of speech to final.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatencyEstimate {
    pub p50_ms: Option<u32>,
    pub p95_ms: Option<u32>,
    pub p99_ms: Option<u32>,
    pub basis: LatencyBasis,
    /// A quantile's rank fell on a timeout, so the figure is a lower bound.
    pub tail_censored: bool,
    pub samples: usize,
}

const FRESH: Duration = Duration::from_secs(30 * 60);
const WINDOW: usize = 512;

#[derive(Debug, Clone, Copy)]
enum Entry {
    Measured(u32),
    /// A request that never answered: slower than every measured sample, with no value.
    RankOnly,
}

#[derive(Debug, Default)]
struct Series {
    entries: VecDeque<(Instant, Entry)>,
}

impl Series {
    fn push(&mut self, at: Instant, e: Entry) {
        if self.entries.len() == WINDOW {
            self.entries.pop_front();
        }
        self.entries.push_back((at, e));
    }

    fn fresh(&self, now: Instant) -> Vec<Entry> {
        self.entries
            .iter()
            .filter(|(at, _)| now.saturating_duration_since(*at) <= FRESH)
            .map(|(_, e)| *e)
            .collect()
    }
}

/// Per-key rolling measurements, in memory per replica.
#[derive(Debug, Default)]
pub struct LatencyStore {
    seeds: Mutex<HashMap<String, Option<u32>>>,
    end_to_final: Mutex<HashMap<String, Series>>,
    round_trip: Mutex<HashMap<String, Series>>,
}

fn nearest_rank(sorted: &[Entry], q: f64) -> (Option<u32>, bool, Option<u32>) {
    // Returns (value, censored, largest measured).
    let largest = sorted.iter().rev().find_map(|e| match e {
        Entry::Measured(v) => Some(*v),
        Entry::RankOnly => None,
    });
    if sorted.is_empty() {
        return (None, false, largest);
    }
    let rank = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    match sorted[rank - 1] {
        Entry::Measured(v) => (Some(v), false, largest),
        Entry::RankOnly => (largest, true, largest),
    }
}

fn sorted(mut entries: Vec<Entry>) -> Vec<Entry> {
    entries.sort_by_key(|e| match e {
        Entry::Measured(v) => (0u8, *v),
        Entry::RankOnly => (1u8, 0),
    });
    entries
}

impl LatencyStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// The seed figure for a key: the row's published end-of-speech-to-final P99, if any.
    pub fn seed(&self, key: &str, p99_ms: Option<u32>) {
        self.seeds.lock().entry(key.to_string()).or_insert(p99_ms);
    }

    pub fn record_end_to_final(&self, key: &str, ms: u32) {
        self.end_to_final
            .lock()
            .entry(key.to_string())
            .or_default()
            .push(Instant::now(), Entry::Measured(ms));
    }

    pub fn record_round_trip(&self, key: &str, ms: u32) {
        self.round_trip
            .lock()
            .entry(key.to_string())
            .or_default()
            .push(Instant::now(), Entry::Measured(ms));
    }

    /// A request that never answered. Stored as a rank-only entry, never as an invented sample,
    /// so stuck requests cannot lengthen any limit.
    pub fn record_timeout(&self, key: &str) {
        let now = Instant::now();
        self.end_to_final
            .lock()
            .entry(key.to_string())
            .or_default()
            .push(now, Entry::RankOnly);
        self.round_trip
            .lock()
            .entry(key.to_string())
            .or_default()
            .push(now, Entry::RankOnly);
    }

    fn estimate_series(&self, seed: Option<u32>, entries: Vec<Entry>) -> LatencyEstimate {
        let n = entries.len();
        let s = sorted(entries);
        let (p50, c50, largest) = nearest_rank(&s, 0.50);
        let (p95, c95, _) = nearest_rank(&s, 0.95);
        let (p99, c99, _) = nearest_rank(&s, 0.99);
        let derived_p50 = seed.map(|v| v / 2);
        let derived_p95 = seed.map(|v| {
            let m = v / 2;
            m + ((v - m) as f64 * 0.4) as u32
        });
        if n < 30 {
            let p99 = match (seed, largest) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            return LatencyEstimate {
                p50_ms: derived_p50,
                p95_ms: derived_p95,
                p99_ms: p99,
                basis: if p99.is_some() {
                    LatencyBasis::Seed
                } else {
                    LatencyBasis::None
                },
                tail_censored: false,
                samples: n,
            };
        }
        if n < 300 {
            let weighted = seed.map(|v| (v as u64 * (300 - n as u64) / 270) as u32);
            let p99 = match (largest, weighted) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            return LatencyEstimate {
                p50_ms: p50,
                p95_ms: p95,
                p99_ms: p99,
                basis: LatencyBasis::Provisional,
                tail_censored: c50 || c95,
                samples: n,
            };
        }
        LatencyEstimate {
            p50_ms: p50,
            p95_ms: p95,
            p99_ms: p99,
            basis: LatencyBasis::Measured,
            tail_censored: c50 || c95 || c99,
            samples: n,
        }
    }

    /// End of speech to final for a key.
    pub fn estimate(&self, key: &str) -> LatencyEstimate {
        let seed = self.seeds.lock().get(key).copied().flatten();
        let entries = self
            .end_to_final
            .lock()
            .get(key)
            .map(|s| s.fresh(Instant::now()))
            .unwrap_or_default();
        self.estimate_series(seed, entries)
    }

    /// The round-trip estimate that drives the limits. The seed is the end-to-final seed less the
    /// published pause; an unmeasured row starts from the global default.
    pub fn round_trip(&self, key: &str) -> LatencyEstimate {
        let seed = self
            .seeds
            .lock()
            .get(key)
            .copied()
            .flatten()
            .unwrap_or(GLOBAL_DEFAULT_P99_MS)
            .saturating_sub(PUBLISHED_PAUSE_MS);
        let entries = self
            .round_trip
            .lock()
            .get(key)
            .map(|s| s.fresh(Instant::now()))
            .unwrap_or_default();
        self.estimate_series(Some(seed), entries)
    }

    /// The limits for one unit of `audio_ms` on this key.
    pub fn deadlines(
        &self,
        key: &str,
        audio_ms: u32,
        deadline_ms: u32,
        row_request_timeout_ms: Option<u32>,
        low_latency: bool,
    ) -> SegmentDeadlines {
        let rt = self.round_trip(key);
        let p99 = rt
            .p99_ms
            .unwrap_or(GLOBAL_DEFAULT_P99_MS - PUBLISHED_PAUSE_MS);
        segment_deadlines(&DeadlineInputs {
            round_trip_p99_ms: p99,
            round_trip_p50_ms: rt.p50_ms.unwrap_or(p99 / 2),
            round_trip_p95_ms: rt.p95_ms.unwrap_or(p99 / 2 + (p99 - p99 / 2) * 2 / 5),
            audio_ms,
            deadline_ms,
            row_request_timeout_ms,
            low_latency,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(p99: u32) -> DeadlineInputs {
        DeadlineInputs {
            round_trip_p99_ms: p99,
            round_trip_p50_ms: p99 / 2,
            round_trip_p95_ms: p99 * 3 / 4,
            audio_ms: 3000,
            deadline_ms: 6000,
            row_request_timeout_ms: Some(10_000),
            low_latency: false,
        }
    }

    #[test]
    fn stall_timeout_is_p99_times_margin_plus_constant_within_floor_and_cap() {
        // ElevenLabs scribe_v2: seed 2,010 ms measured with a 200 ms pause => round trip 1,810.
        assert_eq!(
            segment_deadlines(&inputs(1810)).stall_timeout,
            Duration::from_millis(2513)
        );
        assert_eq!(
            segment_deadlines(&inputs(100)).stall_timeout,
            Duration::from_millis(1500)
        );
        assert_eq!(
            segment_deadlines(&inputs(9000)).stall_timeout,
            Duration::from_millis(3500)
        );
        let long = DeadlineInputs {
            audio_ms: 9000,
            ..inputs(1810)
        };
        assert_eq!(
            segment_deadlines(&long).stall_timeout,
            Duration::from_millis(2713)
        );
    }

    #[test]
    fn the_request_limit_is_the_rows_value_at_most_ten_seconds() {
        let d = segment_deadlines(&DeadlineInputs {
            row_request_timeout_ms: Some(30_000),
            ..inputs(1810)
        });
        assert_eq!(d.request_limit, Duration::from_secs(10));
        let d = segment_deadlines(&DeadlineInputs {
            row_request_timeout_ms: Some(5000),
            ..inputs(1810)
        });
        assert_eq!(d.request_limit, Duration::from_secs(5));
    }

    #[test]
    fn room_for_a_second_request_and_the_hedge_delay() {
        let d = segment_deadlines(&inputs(1810));
        assert_eq!(d.second_request_room, Duration::from_millis(1005));
        assert_eq!(d.hedge_delay, None);
        let d = segment_deadlines(&DeadlineInputs {
            low_latency: true,
            ..inputs(1810)
        });
        assert_eq!(d.hedge_delay, Some(Duration::from_millis(1357)));
        let d = segment_deadlines(&DeadlineInputs {
            low_latency: true,
            round_trip_p95_ms: 100,
            ..inputs(400)
        });
        assert_eq!(d.hedge_delay, Some(Duration::from_millis(300)));
        assert_eq!(d.second_request_room, Duration::from_millis(500));
    }

    #[test]
    fn a_deployment_deadline_below_the_ceiling_plus_2500_is_raised() {
        assert_eq!(effective_deadline_ms(None, 1500), (6000, false));
        assert_eq!(effective_deadline_ms(Some(3000), 1500), (4000, true));
        assert_eq!(effective_deadline_ms(Some(3000), 2900), (5400, true));
        assert_eq!(effective_deadline_ms(Some(8000), 1500), (8000, false));
        assert_eq!(effective_deadline_ms(Some(20_000), 1500), (10_000, true));
    }

    #[test]
    fn the_reported_resolution_deadline_is_6922_by_default() {
        assert_eq!(resolution_deadline_ms(224, 6000), 6922);
    }

    #[test]
    fn an_unmeasured_row_starts_from_the_global_default() {
        let store = LatencyStore::new();
        store.seed("openai:gpt-transcribe", None);
        let rt = store.round_trip("openai:gpt-transcribe");
        assert_eq!(rt.p99_ms, Some(1800));
        let d = store.deadlines("openai:gpt-transcribe", 3000, 6000, None, false);
        assert_eq!(d.stall_timeout, Duration::from_millis(2500));
        assert_eq!(
            store.estimate("openai:gpt-transcribe").basis,
            LatencyBasis::None
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stuck_requests_do_not_move_any_time_limit() {
        let store = LatencyStore::new();
        store.seed("k", Some(2010));
        let before = store.deadlines("k", 3000, 6000, None, false);
        for _ in 0..20 {
            store.record_timeout("k");
        }
        assert_eq!(store.deadlines("k", 3000, 6000, None, false), before);
    }

    #[tokio::test(start_paused = true)]
    async fn estimates_move_from_seed_to_provisional_to_measured() {
        let store = LatencyStore::new();
        store.seed("k", Some(2000));
        for _ in 0..29 {
            store.record_end_to_final("k", 900);
        }
        let e = store.estimate("k");
        assert_eq!((e.basis, e.p99_ms), (LatencyBasis::Seed, Some(2000)));
        store.record_end_to_final("k", 900);
        let e = store.estimate("k");
        assert_eq!(e.basis, LatencyBasis::Provisional);
        assert_eq!(e.p50_ms, Some(900));
        assert_eq!(e.p99_ms, Some(2000), "seed weight 270/270 at n = 30");
        for _ in 0..270 {
            store.record_end_to_final("k", 1000);
        }
        let e = store.estimate("k");
        assert_eq!(e.basis, LatencyBasis::Measured);
        assert_eq!(e.p99_ms, Some(1000));
    }

    #[tokio::test(start_paused = true)]
    async fn a_rank_on_a_timeout_reports_the_largest_measured_value_flagged() {
        let store = LatencyStore::new();
        store.seed("k", Some(1000));
        for _ in 0..290 {
            store.record_end_to_final("k", 800);
        }
        for _ in 0..10 {
            store.record_timeout("k");
        }
        let e = store.estimate("k");
        assert_eq!(e.basis, LatencyBasis::Measured);
        assert_eq!(e.p99_ms, Some(800));
        assert!(e.tail_censored);
    }

    #[tokio::test(start_paused = true)]
    async fn samples_older_than_thirty_minutes_are_not_fresh() {
        let store = LatencyStore::new();
        store.seed("k", Some(1000));
        for _ in 0..40 {
            store.record_end_to_final("k", 700);
        }
        assert_eq!(store.estimate("k").basis, LatencyBasis::Provisional);
        tokio::time::advance(Duration::from_secs(31 * 60)).await;
        assert_eq!(store.estimate("k").basis, LatencyBasis::Seed);
    }

    #[test]
    fn latency_class_thresholds() {
        assert_eq!(
            LatencyClass::of(Some(600), 600, 1200),
            LatencyClass::Realtime
        );
        assert_eq!(LatencyClass::of(Some(601), 600, 1200), LatencyClass::Fast);
        assert_eq!(LatencyClass::of(Some(2010), 600, 1200), LatencyClass::Slow);
        assert_eq!(LatencyClass::of(None, 600, 1200), LatencyClass::Unknown);
    }
}
