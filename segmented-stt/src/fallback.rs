//! Fallback to a second vendor (Release 5).
//!
//! A Bud deployment lists its fallback deployments (`fallback_models`, the list
//! `/v1/audio/transcriptions` already walks). On a live call, a unit the active vendor loses for a
//! reason the next one may not share (an outage, a refused credential, a model that is not served,
//! an open breaker, a spent rate budget) goes again to the next target while the unit's deadline
//! allows, and the session stays there: vendors are not alternated turn by turn. The switch is said
//! once (`stt_fallback_engaged`); `ready.stt` still names the first vendor.
//!
//! A loss the next vendor would share (the audio was refused, a unit outside the vendor's bounds)
//! does not move the session, and neither does one timeout: a run of them opens the breaker, which
//! does.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tokio::time::Instant;

use crate::sequencer::{SegmentUpload, UnitUpload};
use crate::transcriber::attempts::{ServedBy, UnitFailure, UploadResolution};
use crate::transcriber::{SegmentAudio, SegmentContext, SegmentTranscript};
use crate::types::{BillingRule, ErrorClass};

/// The `stt_warning` code.
pub const ENGAGED: &str = "stt_fallback_engaged";

/// One deployment the session may be served by.
pub struct FallbackTarget {
    /// The deployment's id: metering and the warning name it.
    pub name: String,
    pub upload: Arc<dyn SegmentUpload>,
    /// What this target's vendor bills per request.
    pub billing: BillingRule,
}

/// The primary, then its fallbacks in the deployment's order.
pub struct FallbackUpload {
    targets: Vec<FallbackTarget>,
    active: AtomicUsize,
    engaged: AtomicBool,
}

impl FallbackUpload {
    pub fn new(primary: FallbackTarget, fallbacks: Vec<FallbackTarget>) -> Self {
        let mut targets = vec![primary];
        targets.extend(fallbacks);
        Self {
            targets,
            active: AtomicUsize::new(0),
            engaged: AtomicBool::new(false),
        }
    }

    fn current(&self) -> &FallbackTarget {
        &self.targets[self.active.load(Ordering::Acquire)]
    }

    /// The deployment serving the session now.
    pub fn active_name(&self) -> &str {
        &self.current().name
    }
}

/// Why a loss may not be the next vendor's: `None` keeps the session where it is.
pub fn moves_on(result: &Result<SegmentTranscript, UnitFailure>) -> Option<&'static str> {
    match result {
        Ok(_) => None,
        Err(UnitFailure::Vendor(e)) => match e.class {
            ErrorClass::Auth => Some("auth"),
            ErrorClass::RateLimited => Some("rate_limited"),
            ErrorClass::ModelNotServed => Some("model_not_served"),
            ErrorClass::Vendor | ErrorClass::Network | ErrorClass::Protocol => Some("vendor_fault"),
            ErrorClass::EndpointRejected => Some("endpoint_rejected"),
            ErrorClass::BadRequest
            | ErrorClass::Timeout
            | ErrorClass::Cancelled
            | ErrorClass::Internal => None,
        },
        Err(UnitFailure::BreakerOpen) => Some("breaker_open"),
        Err(UnitFailure::LimiterRefused) => Some("capacity"),
        Err(UnitFailure::SessionFatal(_)) => Some("session_fatal"),
        Err(UnitFailure::TimedOut | UnitFailure::Local(_)) => None,
    }
}

#[async_trait::async_trait]
impl SegmentUpload for FallbackUpload {
    async fn run(&self, audio: SegmentAudio, req: UnitUpload) -> UploadResolution {
        let mut i = self.active.load(Ordering::Acquire);
        let mut moved: Option<(usize, &'static str)> = None;
        loop {
            let target = &self.targets[i];
            let mut r = target.upload.run(audio.clone(), req.clone()).await;
            let why = moves_on(&r.result);
            let next = i + 1;
            let stay =
                why.is_none() || next >= self.targets.len() || Instant::now() >= req.deadline_at;
            if !stay {
                // Concurrent units move the session once.
                let _ = self
                    .active
                    .compare_exchange(i, next, Ordering::AcqRel, Ordering::Acquire);
                moved.get_or_insert((i, why.unwrap_or_default()));
                i = next;
                continue;
            }
            if i > 0 {
                r.served_by = Some(ServedBy {
                    name: target.name.clone(),
                    billing: target.billing,
                });
            }
            if let Some((from, why)) = moved
                && !self.engaged.swap(true, Ordering::AcqRel)
            {
                r.warnings.push((
                    ENGAGED.to_string(),
                    format!(
                        "Speech-to-text on '{}' failed ({why}); the fallback deployment '{}' \
                         transcribes the rest of this call.",
                        self.targets[from].name,
                        self.active_name()
                    ),
                ));
            }
            return r;
        }
    }

    fn deadline_ms(&self) -> u32 {
        self.targets[0].upload.deadline_ms()
    }

    fn try_speculative(&self) -> bool {
        self.current().upload.try_speculative()
    }

    async fn prewarm(&self, connections: usize) {
        self.current().upload.prewarm(connections).await;
    }

    fn min_audio_ms(&self) -> u32 {
        self.current().upload.min_audio_ms()
    }

    fn record_end_to_final(&self, ms: u32) {
        self.current().upload.record_end_to_final(ms);
    }

    async fn redecode(&self, audio: SegmentAudio, ctx: SegmentContext) -> Option<String> {
        self.current().upload.redecode(audio, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::time::Duration;

    use parking_lot::Mutex;

    use super::*;
    use crate::transcriber::SegmentError;
    use crate::transcriber::attempts::Ledger;

    /// A target that answers from a script, then with text.
    struct Scripted {
        script: Mutex<VecDeque<Result<SegmentTranscript, UnitFailure>>>,
        calls: AtomicUsize,
        min_audio_ms: u32,
    }

    impl Scripted {
        fn new(script: Vec<Result<SegmentTranscript, UnitFailure>>) -> Arc<Self> {
            Arc::new(Self {
                script: Mutex::new(script.into()),
                calls: AtomicUsize::new(0),
                min_audio_ms: 0,
            })
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    fn text(t: &str) -> Result<SegmentTranscript, UnitFailure> {
        Ok(SegmentTranscript {
            text: t.into(),
            ..Default::default()
        })
    }

    fn vendor(status: u16) -> Result<SegmentTranscript, UnitFailure> {
        Err(UnitFailure::Vendor(SegmentError::from_status(status, "x")))
    }

    #[async_trait::async_trait]
    impl SegmentUpload for Scripted {
        async fn run(&self, audio: SegmentAudio, _req: UnitUpload) -> UploadResolution {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let result = self.script.lock().pop_front().unwrap_or_else(|| text("ok"));
            UploadResolution {
                result,
                ledger: Ledger {
                    requests: 1,
                    uploaded_ms: audio.audio_ms(),
                },
                queue_wait: Duration::ZERO,
                round_trip: None,
                fatal: None,
                warnings: Vec::new(),
                served_by: None,
            }
        }
        fn deadline_ms(&self) -> u32 {
            6000
        }
        fn try_speculative(&self) -> bool {
            true
        }
        async fn prewarm(&self, _connections: usize) {}
        fn min_audio_ms(&self) -> u32 {
            self.min_audio_ms
        }
    }

    fn target(name: &str, s: &Arc<Scripted>, min_billed_ms: u32) -> FallbackTarget {
        FallbackTarget {
            name: name.into(),
            upload: s.clone(),
            billing: BillingRule {
                min_billed_ms,
                increment_ms: 0,
            },
        }
    }

    fn unit() -> (SegmentAudio, UnitUpload) {
        (
            SegmentAudio::new(vec![0; 16_000]),
            UnitUpload {
                ctx: SegmentContext::default(),
                handed_over: Instant::now(),
                deadline_at: Instant::now() + Duration::from_secs(6),
                turn_final: true,
            },
        )
    }

    async fn run(f: &FallbackUpload) -> UploadResolution {
        let (a, r) = unit();
        f.run(a, r).await
    }

    fn texts(r: &UploadResolution) -> Option<String> {
        r.result.as_ref().ok().map(|t| t.text.clone())
    }

    #[tokio::test]
    async fn an_outage_moves_the_unit_and_the_session_to_the_fallback_once() {
        let primary = Scripted::new(vec![vendor(503)]);
        let second = Scripted::new(vec![text("hello"), text("again")]);
        let f = FallbackUpload::new(
            target("primary", &primary, 0),
            vec![target("second", &second, 10_000)],
        );
        let r = run(&f).await;
        assert_eq!(
            texts(&r).as_deref(),
            Some("hello"),
            "the lost unit goes again"
        );
        let served = r.served_by.clone().unwrap();
        assert_eq!(served.name, "second");
        assert_eq!(
            served.billing.min_billed_ms, 10_000,
            "billed by the fallback's rule"
        );
        assert_eq!(r.warnings.len(), 1);
        assert_eq!(r.warnings[0].0, ENGAGED);
        assert!(r.warnings[0].1.contains("'primary' failed (vendor_fault)"));
        assert!(r.warnings[0].1.contains("'second'"));
        assert_eq!(f.active_name(), "second");

        // The session stays on the fallback, and the switch is said once.
        let r = run(&f).await;
        assert_eq!(texts(&r).as_deref(), Some("again"));
        assert!(r.warnings.is_empty());
        assert_eq!(primary.calls(), 1, "the primary is not tried again");
        assert_eq!(second.calls(), 2);
    }

    #[tokio::test]
    async fn every_vendor_side_loss_moves_on_and_an_audio_side_one_does_not() {
        for (loss, moves) in [
            (vendor(401), true),
            (vendor(429), true),
            (vendor(404), true),
            (vendor(500), true),
            (Err(UnitFailure::BreakerOpen), true),
            (Err(UnitFailure::LimiterRefused), true),
            (Err(UnitFailure::SessionFatal(ErrorClass::Auth)), true),
            (vendor(400), false),
            (Err(UnitFailure::TimedOut), false),
            (Err(UnitFailure::Local("too short".into())), false),
        ] {
            let label = format!("{loss:?}");
            let primary = Scripted::new(vec![loss]);
            let second = Scripted::new(vec![]);
            let f = FallbackUpload::new(target("p", &primary, 0), vec![target("s", &second, 0)]);
            let r = run(&f).await;
            assert_eq!(r.result.is_ok(), moves, "{label}");
            assert_eq!(second.calls(), usize::from(moves), "{label}");
            assert_eq!(f.active_name(), if moves { "s" } else { "p" }, "{label}");
        }
    }

    #[tokio::test]
    async fn the_list_is_walked_in_order_and_the_last_loss_is_returned() {
        let a = Scripted::new(vec![vendor(503)]);
        let b = Scripted::new(vec![vendor(401)]);
        let c = Scripted::new(vec![text("third")]);
        let f = FallbackUpload::new(
            target("a", &a, 0),
            vec![target("b", &b, 0), target("c", &c, 0)],
        );
        let r = run(&f).await;
        assert_eq!(texts(&r).as_deref(), Some("third"));
        assert!(r.warnings[0].1.contains("'a' failed"), "{:?}", r.warnings);
        assert!(r.warnings[0].1.contains("'c'"), "{:?}", r.warnings);

        let a = Scripted::new(vec![vendor(503)]);
        let b = Scripted::new(vec![vendor(502)]);
        let f = FallbackUpload::new(target("a", &a, 0), vec![target("b", &b, 0)]);
        let r = run(&f).await;
        assert!(matches!(r.result, Err(UnitFailure::Vendor(_))));
        assert_eq!(r.served_by.unwrap().name, "b");
    }

    #[tokio::test]
    async fn a_unit_past_its_deadline_is_not_sent_again() {
        let primary = Scripted::new(vec![vendor(503)]);
        let second = Scripted::new(vec![]);
        let f = FallbackUpload::new(target("p", &primary, 0), vec![target("s", &second, 0)]);
        let (a, mut req) = unit();
        req.deadline_at = Instant::now();
        let r = f.run(a, req).await;
        assert!(r.result.is_err());
        assert_eq!(second.calls(), 0);
    }

    #[tokio::test]
    async fn without_fallbacks_the_primary_answers_as_before() {
        let primary = Scripted::new(vec![vendor(503)]);
        let f = FallbackUpload::new(target("p", &primary, 0), vec![]);
        let r = run(&f).await;
        assert!(r.result.is_err());
        assert!(r.served_by.is_none() && r.warnings.is_empty());
    }
}
