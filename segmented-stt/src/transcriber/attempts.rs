//! The one attempt loop: at most two requests per upload unit, and every time limit applied.
//!
//! A second request is made in exactly three situations (the fourth, a rotating token, belongs to
//! vendors this crate does not reach): a fast failure worth repeating, a first request that sent no
//! response headers by the stall timeout (the first keeps running and the first answer wins), and a
//! refused optional field the request can be repaired without. Fast failures and stalls spend a
//! retry-budget token. A commit socket, a single-process server and the breaker's half-open probe
//! never get a second request. The loop gives the unit up at its deadline instant.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use parking_lot::Mutex;
use tokio::time::Instant;

use super::breaker::{Admission, FileBreaker};
use super::gate::{Limiter, RetryBudget};
use super::{
    RequestPhase, RequestProgress, SegmentAudio, SegmentContext, SegmentError, SegmentTranscriber,
    SegmentTranscript, TranscriberInfo,
};
use crate::limits::SegmentDeadlines;
use crate::types::ErrorClass;

/// When a second request may be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondRequestPolicy {
    /// No retry (a repair of a refused field is still made).
    Never,
    /// After a fast failure, or when the first request has stalled.
    OnFailureOrStall,
    /// The low-latency tier: as above, and also once the first request has been outstanding for
    /// the hedge delay.
    Hedge,
}

/// One unit's request to the loop.
#[derive(Debug, Clone)]
pub struct UploadRequest {
    pub ctx: SegmentContext,
    pub deadlines: SegmentDeadlines,
    pub handed_over: Instant,
    /// The newest cut of the turn at hand-over plus the deadline: the unit is given up here.
    pub deadline_at: Instant,
}

/// What reached the vendor, readable whatever happened.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ledger {
    pub requests: u8,
    /// Audio handed to the vendor over every request.
    pub uploaded_ms: u32,
}

/// Why a unit failed in the loop.
#[derive(Debug, Clone, PartialEq)]
pub enum UnitFailure {
    Vendor(SegmentError),
    BreakerOpen,
    LimiterRefused,
    TimedOut,
    /// An earlier unit ended the session; nothing was sent.
    SessionFatal(ErrorClass),
    /// The unit was refused before sending (outside the vendor's audio bounds).
    Local(String),
}

#[derive(Debug, Clone)]
pub struct UploadResolution {
    pub result: Result<SegmentTranscript, UnitFailure>,
    pub ledger: Ledger,
    pub queue_wait: Duration,
    /// Round trip of the winning request (first byte to answer).
    pub round_trip: Option<Duration>,
    /// Set when this unit ended the session (a refused credential, a model that is not served).
    pub fatal: Option<ErrorClass>,
    /// `(code, message)` warnings for the client, for example a field the vendor refused.
    pub warnings: Vec<(String, String)>,
}

/// Reports each request's timing, for the latency store.
pub trait AttemptObserver: Send + Sync {
    fn answered(&self, round_trip: Duration);
    /// A request that never answered: recorded as a rank-only entry.
    fn never_answered(&self);
}

/// The session's request history, for the rules that end a session.
#[derive(Debug, Default)]
pub struct SessionHealth {
    inner: Mutex<HealthInner>,
    fatal_flag: AtomicBool,
}

#[derive(Debug, Default)]
struct HealthInner {
    fatal: Option<ErrorClass>,
    any_request: bool,
    ever_success: bool,
    auth_streak: u32,
    refused_streak: u32,
}

impl SessionHealth {
    pub fn fatal(&self) -> Option<ErrorClass> {
        if !self.fatal_flag.load(Ordering::Acquire) {
            return None;
        }
        self.inner.lock().fatal
    }

    fn success(&self) {
        let mut g = self.inner.lock();
        g.any_request = true;
        g.ever_success = true;
        g.auth_streak = 0;
        g.refused_streak = 0;
    }

    /// Apply the escalation rules to a final failure. Returns the fatal class when one fires.
    fn failure(&self, e: &SegmentError) -> Option<ErrorClass> {
        let mut g = self.inner.lock();
        let first = !g.any_request;
        g.any_request = true;
        let fatal = match e.class {
            ErrorClass::ModelNotServed | ErrorClass::EndpointRejected => Some(e.class),
            ErrorClass::Auth => {
                g.auth_streak += 1;
                (first || g.auth_streak >= 2).then_some(ErrorClass::Auth)
            }
            ErrorClass::BadRequest => {
                g.refused_streak += 1;
                let limit = if g.ever_success { 5 } else { 3 };
                (g.refused_streak >= limit).then_some(ErrorClass::BadRequest)
            }
            _ => {
                g.auth_streak = 0;
                None
            }
        };
        if let Some(c) = fatal {
            g.fatal = Some(c);
            self.fatal_flag.store(true, Ordering::Release);
        }
        fatal
    }
}

/// Repairs remembered per host and adapter for ten minutes, so other sessions do not pay the round
/// trip again.
#[derive(Debug, Default)]
pub struct RepairMemory {
    map: Mutex<HashMap<String, (Instant, Repair)>>,
}

#[derive(Debug, Clone, PartialEq)]
enum Repair {
    Omit(Vec<String>),
    Minimal,
}

const REPAIR_TTL: Duration = Duration::from_secs(600);

impl RepairMemory {
    fn key(info: &TranscriberInfo) -> String {
        format!("{}|{}|{}", info.host_key, info.adapter, info.model)
    }

    fn apply(&self, info: &TranscriberInfo, ctx: &mut SegmentContext) {
        let now = Instant::now();
        let mut g = self.map.lock();
        let key = Self::key(info);
        match g.get(&key) {
            Some((at, _)) if now.saturating_duration_since(*at) > REPAIR_TTL => {
                g.remove(&key);
            }
            Some((_, Repair::Omit(fields))) => {
                for f in fields {
                    if !ctx.omit_fields.contains(f) {
                        ctx.omit_fields.push(f.clone());
                    }
                }
            }
            Some((_, Repair::Minimal)) => ctx.minimal = true,
            None => {}
        }
    }

    fn remember(&self, info: &TranscriberInfo, repair: Repair) {
        self.map
            .lock()
            .insert(Self::key(info), (Instant::now(), repair));
    }
}

/// The loop over one target.
pub struct SegmentAttempts {
    pub transcriber: Arc<dyn SegmentTranscriber>,
    pub breaker: Arc<FileBreaker>,
    pub limiter: Arc<Limiter>,
    pub budget: Arc<RetryBudget>,
    pub policy: SecondRequestPolicy,
    pub health: Arc<SessionHealth>,
    pub repairs: Arc<RepairMemory>,
    pub observer: Option<Arc<dyn AttemptObserver>>,
}

type ReqFuture = Pin<
    Box<
        dyn std::future::Future<
                Output = (
                    u8,
                    Result<SegmentTranscript, SegmentError>,
                    Arc<RequestProgress>,
                    Instant,
                ),
            > + Send,
    >,
>;

/// Records `None` on the breaker if the unit is dropped before its outcome is known.
struct PermitGuard<'a> {
    breaker: &'a FileBreaker,
    admission: Admission,
    done: bool,
}

impl PermitGuard<'_> {
    fn resolve(&mut self, outcome: Option<bool>) {
        if !self.done {
            self.breaker.record(self.admission, outcome);
            self.done = true;
        }
    }
}

impl Drop for PermitGuard<'_> {
    fn drop(&mut self) {
        self.resolve(None);
    }
}

fn retry_wait(retry_after: Option<Duration>) -> Duration {
    // The larger of the vendor's Retry-After (honoured up to one second) and 100 to 400 ms.
    let jitter = 100
        + (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
            % 300) as u64;
    retry_after
        .unwrap_or(Duration::ZERO)
        .min(Duration::from_secs(1))
        .max(Duration::from_millis(jitter))
}

impl SegmentAttempts {
    pub fn info(&self) -> &TranscriberInfo {
        self.transcriber.info()
    }

    fn spawn(
        &self,
        n: u8,
        audio: &SegmentAudio,
        ctx: &SegmentContext,
        limit: Duration,
        progress: Arc<RequestProgress>,
    ) -> ReqFuture {
        let t = Arc::clone(&self.transcriber);
        let audio = audio.clone();
        let ctx = ctx.clone();
        Box::pin(async move {
            let started = Instant::now();
            let r = match tokio::time::timeout(limit, t.transcribe(&audio, &ctx, limit, &progress))
                .await
            {
                Ok(r) => r,
                Err(_) => Err(
                    SegmentError::new(ErrorClass::Timeout, "request limit reached").with_phase(
                        if progress.headers_received() {
                            RequestPhase::Headers
                        } else {
                            RequestPhase::Sent
                        },
                    ),
                ),
            };
            (n, r, progress, started)
        })
    }

    /// One call per upload unit. Dropping the returned future cancels everything in flight.
    pub async fn run(&self, audio: SegmentAudio, req: UploadRequest) -> UploadResolution {
        let mut ledger = Ledger::default();
        let mut warnings = Vec::new();
        let resolution =
            |result, ledger, queue_wait, round_trip, fatal, warnings| UploadResolution {
                result,
                ledger,
                queue_wait,
                round_trip,
                fatal,
                warnings,
            };
        if let Some(c) = self.health.fatal() {
            return resolution(
                Err(UnitFailure::SessionFatal(c)),
                ledger,
                Duration::ZERO,
                None,
                None,
                warnings,
            );
        }
        let info = self.transcriber.info().clone();
        let audio_ms = audio.audio_ms();
        if let Some(max) = info.max_audio_ms
            && audio_ms > max
        {
            return resolution(
                Err(UnitFailure::Local(format!(
                    "{audio_ms} ms is over the vendor's {max} ms"
                ))),
                ledger,
                Duration::ZERO,
                None,
                None,
                warnings,
            );
        }
        if let Some(max) = info.max_upload_bytes
            && (audio.pcm.len() as u64 * 2 + 44) > max
        {
            return resolution(
                Err(UnitFailure::Local(
                    "the upload is over the vendor's size limit".into(),
                )),
                ledger,
                Duration::ZERO,
                None,
                None,
                warnings,
            );
        }
        if Instant::now() >= req.deadline_at {
            return resolution(
                Err(UnitFailure::TimedOut),
                ledger,
                Duration::ZERO,
                None,
                None,
                warnings,
            );
        }
        let admission = self.breaker.try_acquire(true);
        if admission == Admission::Denied {
            return resolution(
                Err(UnitFailure::BreakerOpen),
                ledger,
                Duration::ZERO,
                None,
                None,
                warnings,
            );
        }
        let mut permit = PermitGuard {
            breaker: &self.breaker,
            admission,
            done: false,
        };
        let is_probe = matches!(admission, Admission::Probe { .. });
        let latest = (req.handed_over + req.deadlines.queue_allowance).min(req.deadline_at);
        let gate_start = Instant::now();
        let pass = match self.limiter.acquire(latest).await {
            Ok(p) => p,
            Err(_) => {
                permit.resolve(None);
                return resolution(
                    Err(UnitFailure::LimiterRefused),
                    ledger,
                    gate_start.elapsed(),
                    None,
                    None,
                    warnings,
                );
            }
        };
        let queue_wait = pass.waited;
        let mut passes = vec![pass];

        let mut ctx = req.ctx.clone();
        self.repairs.apply(&info, &mut ctx);
        let limit = req.deadlines.request_limit;
        let can_second = info.allows_second_request() && !is_probe;
        let mut inflight: FuturesUnordered<ReqFuture> = FuturesUnordered::new();
        self.budget.on_request();
        let first_progress = RequestProgress::new();
        inflight.push(self.spawn(1, &audio, &ctx, limit, Arc::clone(&first_progress)));
        ledger.requests = 1;
        ledger.uploaded_ms = audio_ms;
        let first_sent = Instant::now();
        let mut second_sent = false;
        let mut stall_checked = false;
        let stall_at = first_sent + req.deadlines.stall_timeout;
        let hedge_at = match (self.policy, req.deadlines.hedge_delay) {
            (SecondRequestPolicy::Hedge, Some(d)) => Some(first_sent + d),
            _ => None,
        };

        let e = loop {
            let timer_at = if second_sent
                || stall_checked
                || !can_second
                || self.policy == SecondRequestPolicy::Never
            {
                None
            } else {
                Some(hedge_at.map_or(stall_at, |h| h.min(stall_at)))
            };
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(req.deadline_at) => {
                    drop(inflight);
                    if let Some(o) = &self.observer { o.never_answered(); }
                    let headers = first_progress.headers_received();
                    permit.resolve(if headers { None } else { Some(false) });
                    return resolution(Err(UnitFailure::TimedOut), ledger, queue_wait, None, None, warnings);
                }
                Some((_n, result, progress, started)) = inflight.next() => {
                    match result {
                        Ok(t) => {
                            let rt = Instant::now().saturating_duration_since(started);
                            if let Some(o) = &self.observer { o.answered(rt); }
                            permit.resolve(Some(true));
                            self.health.success();
                            return resolution(Ok(t), ledger, queue_wait, Some(rt), None, warnings);
                        }
                        Err(e) => {
                            if e.class == ErrorClass::RateLimited {
                                self.limiter.observe_rate_limited(e.retry_after);
                            }
                            if e.class == ErrorClass::Timeout && !progress.headers_received()
                                && let Some(o) = &self.observer {
                                o.never_answered();
                            }
                            if !inflight.is_empty() {
                                continue; // the other request may still answer
                            }
                            // Decide a second request.
                            if !second_sent {
                                if let Some((repaired, repair)) = repair_for(&info, &ctx, &e) {
                                    let latest = Instant::now().max(req.deadline_at.checked_sub(req.deadlines.second_request_room).unwrap_or(req.deadline_at));
                                    if let Ok(p) = self.limiter.acquire(latest).await {
                                        passes.push(p);
                                        second_sent = true;
                                        ledger.requests += 1;
                                        ledger.uploaded_ms += audio_ms;
                                        self.repairs.remember(&info, repair.clone());
                                        let not_sent = match &repair { Repair::Omit(f) => f.join(", "), Repair::Minimal => "all optional fields".into() };
                                        warnings.push(("stt_fields_reduced".into(), format!("The vendor refused a request field; sent without: {not_sent}")));
                                        ctx = repaired;
                                        inflight.push(self.spawn(2, &audio, &ctx, limit, RequestProgress::new()));
                                        continue;
                                    }
                                }
                                let room_left = req.deadline_at.saturating_duration_since(Instant::now()) >= req.deadlines.second_request_room;
                                if e.is_fast_retryable() && can_second && self.policy != SecondRequestPolicy::Never
                                    && room_left && self.budget.try_spend()
                                {
                                    tokio::time::sleep(retry_wait(e.retry_after)).await;
                                    let latest = req.deadline_at.checked_sub(req.deadlines.second_request_room).unwrap_or(req.deadline_at).max(Instant::now());
                                    if let Ok(p) = self.limiter.acquire(latest).await {
                                        passes.push(p);
                                        second_sent = true;
                                        ledger.requests += 1;
                                        ledger.uploaded_ms += audio_ms;
                                        inflight.push(self.spawn(2, &audio, &ctx, limit, RequestProgress::new()));
                                        continue;
                                    }
                                }
                            }
                            break e;
                        }
                    }
                }
                _ = async { match timer_at { Some(t) => tokio::time::sleep_until(t).await, None => std::future::pending().await } } => {
                    stall_checked = true;
                    let hedge = hedge_at.is_some_and(|h| Instant::now() >= h && Instant::now() < stall_at);
                    let stalled = !first_progress.headers_received();
                    let room_left = req.deadline_at.saturating_duration_since(Instant::now()) >= req.deadlines.second_request_room;
                    if (hedge || stalled) && room_left && !inflight.is_empty() && self.budget.try_spend() {
                        let latest = Instant::now().max(req.deadline_at.checked_sub(req.deadlines.second_request_room).unwrap_or(req.deadline_at));
                        if let Ok(p) = self.limiter.acquire(latest).await {
                            passes.push(p);
                            second_sent = true;
                            ledger.requests += 1;
                            ledger.uploaded_ms += audio_ms;
                            inflight.push(self.spawn(2, &audio, &ctx, limit, RequestProgress::new()));
                        }
                    }
                }
            }
        };

        drop(passes);
        permit.resolve(e.counts_for_breaker().then_some(false));
        if e.class == ErrorClass::Timeout {
            return resolution(
                Err(UnitFailure::TimedOut),
                ledger,
                queue_wait,
                None,
                None,
                warnings,
            );
        }
        let fatal = self.health.failure(&e);
        resolution(
            Err(UnitFailure::Vendor(e)),
            ledger,
            queue_wait,
            None,
            fatal,
            warnings,
        )
    }
}

/// A refused request that leaving out optional fields can repair: the field the vendor named, if
/// the target may drop it; otherwise, when the refusal names no field and optional fields were
/// sent, only the file and the model.
fn repair_for(
    info: &TranscriberInfo,
    ctx: &SegmentContext,
    e: &SegmentError,
) -> Option<(SegmentContext, Repair)> {
    if e.class != ErrorClass::BadRequest || ctx.minimal {
        return None;
    }
    match &e.refused_field {
        Some(field)
            if info.droppable_fields.iter().any(|f| f == field)
                && !ctx.omit_fields.contains(field) =>
        {
            let mut c = ctx.clone();
            c.omit_fields.push(field.clone());
            let fields = c.omit_fields.clone();
            Some((c, Repair::Omit(fields)))
        }
        Some(_) => None,
        None => {
            let mut c = ctx.clone();
            c.minimal = true;
            Some((c, Repair::Minimal))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::{DeadlineInputs, segment_deadlines};
    use crate::transcriber::TranscriberKind;
    use crate::transcriber::breaker::{BreakerConfig, BreakerState};
    use crate::transcriber::gate::LimitSpec;
    use crate::transcriber::testing::{FakeStep, FakeTranscriber};

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    fn deadlines() -> SegmentDeadlines {
        segment_deadlines(&DeadlineInputs {
            round_trip_p99_ms: 1810,
            round_trip_p50_ms: 800,
            round_trip_p95_ms: 1200,
            audio_ms: 3000,
            deadline_ms: 6000,
            row_request_timeout_ms: Some(10_000),
            low_latency: false,
        })
    }

    fn attempts(t: Arc<FakeTranscriber>) -> SegmentAttempts {
        SegmentAttempts {
            transcriber: t,
            breaker: Arc::new(FileBreaker::new(BreakerConfig::default())),
            limiter: Arc::new(Limiter::new(LimitSpec::unlimited())),
            budget: Arc::new(RetryBudget::default()),
            policy: SecondRequestPolicy::OnFailureOrStall,
            health: Arc::new(SessionHealth::default()),
            repairs: Arc::new(RepairMemory::default()),
            observer: None,
        }
    }

    fn request(d: SegmentDeadlines) -> UploadRequest {
        let now = Instant::now();
        UploadRequest {
            ctx: SegmentContext {
                language: Some("en".into()),
                prompt: Some("acme".into()),
                ..Default::default()
            },
            deadlines: d,
            handed_over: now,
            deadline_at: now + d.deadline,
        }
    }

    fn audio() -> SegmentAudio {
        SegmentAudio::new(vec![0; 16_000])
    }

    #[tokio::test(start_paused = true)]
    async fn a_transcript_on_the_first_request_is_one_request() {
        let t = FakeTranscriber::file([FakeStep::text("hello", ms(800))]);
        let a = attempts(Arc::clone(&t));
        let r = a.run(audio(), request(deadlines())).await;
        assert_eq!(r.result.unwrap().text, "hello");
        assert_eq!(
            r.ledger,
            Ledger {
                requests: 1,
                uploaded_ms: 1000
            }
        );
        assert_eq!(r.round_trip, Some(ms(800)));
        assert_eq!(t.calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_request_gets_a_second_request_and_the_first_is_not_cancelled() {
        // The first never sends headers; the second answers 800 ms after the 2,513 ms stall.
        let t = FakeTranscriber::file([FakeStep::stall(), FakeStep::text("second", ms(800))]);
        let a = attempts(Arc::clone(&t));
        let start = Instant::now();
        let r = a.run(audio(), request(deadlines())).await;
        assert_eq!(r.result.unwrap().text, "second");
        assert_eq!(r.ledger.requests, 2);
        assert_eq!(t.max_in_flight(), 2, "both requests were running together");
        let took = Instant::now() - start;
        assert!(took >= ms(3300) && took < ms(3400), "{took:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn one_utterance_never_produces_more_than_two_vendor_requests() {
        let t = FakeTranscriber::file([
            FakeStep::error(SegmentError::from_status(503, "busy"), ms(100)),
            FakeStep::error(SegmentError::from_status(503, "busy"), ms(100)),
            FakeStep::text("never", ms(100)),
        ]);
        let a = attempts(Arc::clone(&t));
        let r = a.run(audio(), request(deadlines())).await;
        assert!(matches!(r.result, Err(UnitFailure::Vendor(_))));
        assert_eq!(t.calls(), 2);
        assert_eq!(r.ledger.requests, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_fast_failure_is_retried_once_after_a_short_wait() {
        let t = FakeTranscriber::file([
            FakeStep::error(SegmentError::from_status(502, "bad gateway"), ms(50)),
            FakeStep::text("ok", ms(500)),
        ]);
        let a = attempts(Arc::clone(&t));
        let r = a.run(audio(), request(deadlines())).await;
        assert_eq!(r.result.unwrap().text, "ok");
        assert_eq!(t.calls(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_commit_socket_never_gets_a_second_request_even_when_stalled() {
        let mut info = TranscriberInfo::file(
            "openai_realtime_transcription",
            "wss://api.openai.com:443",
            "gpt-live-transcribe",
        );
        info.kind = TranscriberKind::Commit;
        let t = FakeTranscriber::new(
            info,
            [
                FakeStep::stall(),
                FakeStep::text("would be a second commit", ms(10)),
            ],
        );
        let a = attempts(Arc::clone(&t));
        let r = a.run(audio(), request(deadlines())).await;
        assert_eq!(r.result.unwrap_err(), UnitFailure::TimedOut);
        assert_eq!(t.calls(), 1, "no second commit on the socket");
    }

    #[tokio::test(start_paused = true)]
    async fn a_commit_socket_that_fails_fast_is_not_retried() {
        let mut info = TranscriberInfo::file(
            "openai_realtime_transcription",
            "wss://api.openai.com:443",
            "gpt-live-transcribe",
        );
        info.kind = TranscriberKind::Commit;
        let t = FakeTranscriber::new(
            info,
            [FakeStep::error(
                SegmentError::new(ErrorClass::Network, "socket dropped"),
                ms(10),
            )],
        );
        let a = attempts(Arc::clone(&t));
        let r = a.run(audio(), request(deadlines())).await;
        assert!(matches!(r.result, Err(UnitFailure::Vendor(_))));
        assert_eq!(t.calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_stuck_vendor_releases_the_unit_at_its_deadline() {
        let t = FakeTranscriber::file([FakeStep::stall(), FakeStep::stall()]);
        let a = attempts(Arc::clone(&t));
        let start = Instant::now();
        let r = a.run(audio(), request(deadlines())).await;
        assert_eq!(r.result.unwrap_err(), UnitFailure::TimedOut);
        assert_eq!(Instant::now() - start, ms(6000));
        assert_eq!(t.calls(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn no_second_request_without_room_before_the_deadline() {
        // Headers come at once (no stall), the failure only at 5,200 ms: 800 ms is left, under the
        // 1,005 ms a second request needs.
        let t = FakeTranscriber::file([
            FakeStep {
                headers_after: Some(ms(100)),
                answer_after: ms(5200),
                result: Err(SegmentError::from_status(503, "")),
            },
            FakeStep::text("late", ms(10)),
        ]);
        let a = attempts(Arc::clone(&t));
        let r = a.run(audio(), request(deadlines())).await;
        assert!(matches!(r.result, Err(UnitFailure::Vendor(_))));
        assert_eq!(t.calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_refused_optional_field_is_dropped_once_and_remembered() {
        let mut refused = SegmentError::from_status(400, "unknown param");
        refused.refused_field = Some("prompt".into());
        let mut info = TranscriberInfo::file("openai_transcriptions", "https://x:443", "m");
        info.droppable_fields = vec!["prompt".into(), "language".into()];
        let t = FakeTranscriber::new(
            info,
            [
                FakeStep::error(refused, ms(50)),
                FakeStep::text("ok", ms(50)),
            ],
        );
        let a = attempts(Arc::clone(&t));
        let r = a.run(audio(), request(deadlines())).await;
        assert_eq!(r.result.unwrap().text, "ok");
        assert_eq!(r.warnings[0].0, "stt_fields_reduced");
        assert_eq!(t.contexts()[1].omit_fields, vec!["prompt".to_string()]);
        // The next unit starts without the field.
        t.push(FakeStep::text("again", ms(50)));
        let r = a.run(audio(), request(deadlines())).await;
        assert_eq!(r.result.unwrap().text, "again");
        assert_eq!(t.contexts()[2].omit_fields, vec!["prompt".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_refusal_naming_no_field_is_repaired_with_a_minimal_request() {
        let t = FakeTranscriber::file([
            FakeStep::error(SegmentError::from_status(422, "invalid"), ms(50)),
            FakeStep::text("ok", ms(50)),
        ]);
        let a = attempts(Arc::clone(&t));
        let r = a.run(audio(), request(deadlines())).await;
        assert_eq!(r.result.unwrap().text, "ok");
        assert!(t.contexts()[1].minimal);
    }

    #[tokio::test(start_paused = true)]
    async fn a_refused_credential_on_the_first_request_ends_the_session() {
        let t = FakeTranscriber::file([FakeStep::error(
            SegmentError::from_status(401, "bad key"),
            ms(50),
        )]);
        let a = attempts(Arc::clone(&t));
        let r = a.run(audio(), request(deadlines())).await;
        assert_eq!(r.fatal, Some(ErrorClass::Auth));
        let r = a.run(audio(), request(deadlines())).await;
        assert_eq!(
            r.result.unwrap_err(),
            UnitFailure::SessionFatal(ErrorClass::Auth)
        );
        assert_eq!(t.calls(), 1, "nothing more is sent");
    }

    #[tokio::test(start_paused = true)]
    async fn one_refused_credential_after_success_does_not_end_the_call() {
        let t = FakeTranscriber::file([
            FakeStep::text("ok", ms(50)),
            FakeStep::error(SegmentError::from_status(403, ""), ms(50)),
            FakeStep::error(SegmentError::from_status(403, ""), ms(50)),
        ]);
        let a = attempts(Arc::clone(&t));
        assert!(a.run(audio(), request(deadlines())).await.result.is_ok());
        assert_eq!(a.run(audio(), request(deadlines())).await.fatal, None);
        assert_eq!(
            a.run(audio(), request(deadlines())).await.fatal,
            Some(ErrorClass::Auth)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_model_that_is_not_served_ends_the_session_at_once() {
        let t = FakeTranscriber::file([FakeStep::error(
            SegmentError::from_status(404, "model"),
            ms(50),
        )]);
        let a = attempts(t);
        assert_eq!(
            a.run(audio(), request(deadlines())).await.fatal,
            Some(ErrorClass::ModelNotServed)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn three_refused_segments_with_no_success_end_the_session() {
        let mut steps = Vec::new();
        for _ in 0..6 {
            let mut e = SegmentError::from_status(400, "");
            e.refused_field = Some("not-droppable".into());
            steps.push(FakeStep::error(e, ms(10)));
        }
        let a = attempts(FakeTranscriber::file(steps));
        assert_eq!(a.run(audio(), request(deadlines())).await.fatal, None);
        assert_eq!(a.run(audio(), request(deadlines())).await.fatal, None);
        assert_eq!(
            a.run(audio(), request(deadlines())).await.fatal,
            Some(ErrorClass::BadRequest)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limit_slows_the_limiter_and_never_opens_the_breaker() {
        let t = FakeTranscriber::file([
            FakeStep::error(
                SegmentError::from_status(429, "slow down").with_retry_after(Some(ms(500))),
                ms(10),
            ),
            FakeStep::text("ok", ms(10)),
        ]);
        let a = attempts(Arc::clone(&t));
        let r = a.run(audio(), request(deadlines())).await;
        assert_eq!(r.result.unwrap().text, "ok");
        assert_eq!(a.breaker.state(), BreakerState::Closed);
    }

    #[tokio::test(start_paused = true)]
    async fn the_limiter_refuses_after_the_queue_allowance() {
        let t = FakeTranscriber::file([]);
        let mut a = attempts(Arc::clone(&t));
        a.limiter = Arc::new(Limiter::new(LimitSpec {
            requests_per_minute: 1.0,
            burst: 1.0,
            max_concurrent: 4,
        }));
        let _ = a.run(audio(), request(deadlines())).await;
        let start = Instant::now();
        let r = a.run(audio(), request(deadlines())).await;
        assert_eq!(r.result.unwrap_err(), UnitFailure::LimiterRefused);
        assert!(Instant::now() - start <= ms(1500));
        assert_eq!(t.calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn an_open_breaker_fails_fast_without_a_request() {
        let t = FakeTranscriber::file([]);
        let a = attempts(Arc::clone(&t));
        a.breaker.force_open();
        let r = a.run(audio(), request(deadlines())).await;
        assert_eq!(r.result.unwrap_err(), UnitFailure::BreakerOpen);
        assert_eq!(t.calls(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn the_low_latency_tier_hedges_at_the_p95() {
        let mut d = deadlines();
        d.hedge_delay = Some(ms(1200));
        let t = FakeTranscriber::file([
            FakeStep::text("slow", ms(5000)),
            FakeStep::text("hedged", ms(500)),
        ]);
        let mut a = attempts(Arc::clone(&t));
        a.policy = SecondRequestPolicy::Hedge;
        let start = Instant::now();
        let r = a.run(audio(), request(d)).await;
        assert_eq!(r.result.unwrap().text, "hedged");
        assert_eq!(Instant::now() - start, ms(1700));
    }

    #[tokio::test(start_paused = true)]
    async fn the_policy_never_sends_no_retry() {
        let t =
            FakeTranscriber::file([FakeStep::error(SegmentError::from_status(503, ""), ms(10))]);
        let mut a = attempts(Arc::clone(&t));
        a.policy = SecondRequestPolicy::Never;
        assert!(a.run(audio(), request(deadlines())).await.result.is_err());
        assert_eq!(t.calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn audio_over_the_vendor_maximum_is_refused_locally() {
        let mut info = TranscriberInfo::file("x", "https://x:443", "m");
        info.max_audio_ms = Some(500);
        let t = FakeTranscriber::new(info, []);
        let a = attempts(Arc::clone(&t));
        assert!(matches!(
            a.run(audio(), request(deadlines())).await.result,
            Err(UnitFailure::Local(_))
        ));
        assert_eq!(t.calls(), 0);
    }
}
