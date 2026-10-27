//! Upload units: the seam the engine calls once per unit, and the joining of held segments.
//!
//! The engine keeps the units and turns themselves (it is their single owner); this module holds
//! what is independent of the engine task: the [`SegmentUpload`] seam, its production form over the
//! attempt loop and the latency store, and the join rule.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use crate::limits::{LatencyStore, MAX_REQUEST_LIMIT_MS};
use crate::segmenter::Segment;
use crate::transcriber::SegmentAudio;
use crate::transcriber::SegmentContext;
use crate::transcriber::attempts::{SegmentAttempts, UploadRequest, UploadResolution};
use crate::transcriber::gate::Limiter;
use crate::types::ms_to_samples;

/// One unit handed to the upload component.
#[derive(Debug, Clone)]
pub struct UnitUpload {
    pub ctx: SegmentContext,
    pub handed_over: Instant,
    /// The newest cut of the turn at hand-over plus the deadline.
    pub deadline_at: Instant,
    pub turn_final: bool,
}

/// The seam to the component that sends a unit to the vendor. The engine calls `run` exactly once
/// per unit and never retries; the component returns by the deadline.
#[async_trait::async_trait]
pub trait SegmentUpload: Send + Sync {
    async fn run(&self, audio: SegmentAudio, req: UnitUpload) -> UploadResolution;
    /// The deadline in force, ms, counted from the turn's newest cut.
    fn deadline_ms(&self) -> u32;
    /// The deadline of a unit starting now: the deadline in force, or longer while the target is
    /// cold (the operator's cold-start allowance).
    fn unit_deadline_ms(&self) -> u32 {
        self.deadline_ms()
    }
    /// May a unit that is not the turn's last start now? Never waits.
    fn try_speculative(&self) -> bool;
    async fn prewarm(&self, connections: usize);
    /// The vendor's shortest accepted upload.
    fn min_audio_ms(&self) -> u32;
    /// End of speech to the release of the unit's text.
    fn record_end_to_final(&self, _ms: u32) {}
    /// Re-decode the open segment for interim text (Release 6): one request, no retry, only when
    /// the limiter has room. `None` when the target offers no re-decoding or it failed.
    async fn redecode(&self, _audio: SegmentAudio, _ctx: SegmentContext) -> Option<String> {
        None
    }
}

/// The production upload: the attempt loop, with limits from the latency store.
pub struct AttemptsUpload {
    pub attempts: SegmentAttempts,
    pub store: Arc<LatencyStore>,
    pub key: String,
    pub deadline_ms: u32,
    pub row_request_timeout_ms: Option<u32>,
    pub low_latency: bool,
    pub limiter: Arc<Limiter>,
    /// The operator's ceiling on one request (10,000 ms unless raised).
    pub max_request_limit_ms: u32,
    /// The operator's cold-start allowance, ms; `0` is off.
    pub cold_start_ms: u32,
    /// The target's identity for the cold-start rule: host and model, never a row shared by
    /// several servers.
    pub target_key: String,
    /// Whether to open warm connections ahead of uploads. The gateway turns it off for a target
    /// on this host or a private network (`net::is_local_origin`), where a connection costs
    /// nothing and the warm request only adds a line to the server's error log.
    pub prewarm: bool,
}

impl AttemptsUpload {
    /// Whether the next unit gets the cold-start allowance.
    fn cold(&self) -> bool {
        self.cold_start_ms > 0 && !self.store.answered_recently(&self.target_key)
    }
}

#[async_trait::async_trait]
impl SegmentUpload for AttemptsUpload {
    async fn run(&self, audio: SegmentAudio, req: UnitUpload) -> UploadResolution {
        let mut deadlines = self.store.deadlines(
            &self.key,
            audio.audio_ms(),
            self.deadline_ms,
            self.row_request_timeout_ms,
            self.low_latency,
        );
        if self.max_request_limit_ms > MAX_REQUEST_LIMIT_MS {
            let asked = self.row_request_timeout_ms.unwrap_or(MAX_REQUEST_LIMIT_MS);
            deadlines.request_limit =
                Duration::from_millis(u64::from(asked.min(self.max_request_limit_ms)));
        }
        // A model loading on demand answers its first request late: one long request, no
        // second request beside it (it would queue behind the load), and the engine's deadline
        // for the unit was lengthened to match (`unit_deadline_ms`).
        let cold = self.cold();
        if cold {
            let allowance = Duration::from_millis(u64::from(self.cold_start_ms));
            deadlines.request_limit = deadlines.request_limit.max(allowance);
            deadlines.stall_timeout = deadlines.stall_timeout.max(allowance);
            deadlines.hedge_delay = None;
            deadlines.deadline = deadlines.deadline.max(allowance);
        }
        let r = self
            .attempts
            .run(
                audio,
                UploadRequest {
                    ctx: req.ctx,
                    deadlines,
                    handed_over: req.handed_over,
                    deadline_at: req.deadline_at,
                },
            )
            .await;
        if let Some(rt) = r.round_trip {
            self.store.mark_answered(&self.target_key);
            // A load time is not a round trip: it would lengthen every limit for 30 minutes.
            if !cold {
                self.store
                    .record_round_trip(&self.key, rt.as_millis() as u32);
            }
        }
        if matches!(
            r.result,
            Err(crate::transcriber::attempts::UnitFailure::TimedOut)
        ) {
            self.store.record_timeout(&self.key);
        }
        r
    }

    fn deadline_ms(&self) -> u32 {
        self.deadline_ms
    }

    fn unit_deadline_ms(&self) -> u32 {
        if self.cold() {
            self.deadline_ms.max(self.cold_start_ms)
        } else {
            self.deadline_ms
        }
    }

    fn try_speculative(&self) -> bool {
        self.limiter.has_headroom()
    }

    async fn prewarm(&self, connections: usize) {
        if self.prewarm {
            self.attempts.transcriber.prewarm(connections).await;
        }
    }

    fn min_audio_ms(&self) -> u32 {
        self.attempts.info().min_audio_ms.unwrap_or(0)
    }

    fn record_end_to_final(&self, ms: u32) {
        self.store.record_end_to_final(&self.key, ms);
    }

    async fn redecode(&self, audio: SegmentAudio, ctx: SegmentContext) -> Option<String> {
        self.attempts.run_interim(audio, ctx).await
    }
}

/// Join the segments of one unit: each segment's real audio, silence for any gap between two
/// (capped), and the last segment's trailing zeros once.
pub fn join_segments(segments: &[Segment], gap_cap_ms: u32) -> Vec<i16> {
    let mut out: Vec<i16> = Vec::new();
    let mut end: Option<u64> = None;
    for s in segments {
        match end {
            None => out.extend_from_slice(&s.pcm),
            Some(prev_end) => {
                if s.audio_start_sample > prev_end {
                    let gap =
                        (s.audio_start_sample - prev_end).min(ms_to_samples(gap_cap_ms as u64));
                    out.resize(out.len() + gap as usize, 0);
                    out.extend_from_slice(&s.pcm);
                } else {
                    let skip = ((prev_end - s.audio_start_sample) as usize).min(s.pcm.len());
                    out.extend_from_slice(&s.pcm[skip..]);
                }
            }
        }
        end = Some(end.map_or(s.audio_end_sample, |e: u64| e.max(s.audio_end_sample)));
    }
    if let Some(last) = segments.last() {
        out.resize(out.len() + last.trailing_zero_samples as usize, 0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::breaker::{Breaker, BreakerConfig};
    use crate::transcriber::attempts::{RepairMemory, SecondRequestPolicy, SessionHealth};
    use crate::transcriber::gate::{LimitSpec, RetryBudget};
    use crate::transcriber::testing::{FakeStep, FakeTranscriber};
    use crate::types::CutReason;

    fn upload(t: Arc<FakeTranscriber>, cold_start_ms: u32) -> AttemptsUpload {
        let limiter = Arc::new(Limiter::new(LimitSpec::unlimited()));
        AttemptsUpload {
            attempts: SegmentAttempts {
                transcriber: t,
                breaker: Arc::new(Breaker::new(BreakerConfig::default())),
                limiter: Arc::clone(&limiter),
                budget: Arc::new(RetryBudget::default()),
                policy: SecondRequestPolicy::OnFailureOrStall,
                health: Arc::new(SessionHealth::default()),
                repairs: Arc::new(RepairMemory::default()),
                observer: None,
            },
            store: Arc::new(LatencyStore::new()),
            key: "openai:any".into(),
            deadline_ms: 6000,
            row_request_timeout_ms: Some(10_000),
            low_latency: false,
            limiter,
            max_request_limit_ms: MAX_REQUEST_LIMIT_MS,
            cold_start_ms,
            target_key: "http://infer:8000|gaia-transcribe".into(),
            prewarm: true,
        }
    }

    fn unit(deadline_ms: u32) -> UnitUpload {
        let now = Instant::now();
        UnitUpload {
            ctx: SegmentContext::default(),
            handed_over: now,
            deadline_at: now + Duration::from_millis(u64::from(deadline_ms)),
            turn_final: true,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn today_a_model_loading_for_forty_seconds_loses_its_first_utterance() {
        let t = FakeTranscriber::file([FakeStep::text("loaded", Duration::from_secs(40))]);
        let up = upload(Arc::clone(&t), 0);
        assert_eq!(up.unit_deadline_ms(), 6000);
        let r = up.run(SegmentAudio::new(vec![0; 16_000]), unit(6000)).await;
        // The stall fires a second request beside the load; whatever it answers (here: nothing, as
        // a request queued behind a load would), the utterance the loading model was transcribing
        // never reaches the caller.
        assert!(
            !matches!(&r.result, Ok(t) if t.text == "loaded"),
            "the loading model's answer arrived: {:?}",
            r.result
        );
        assert_eq!(
            t.calls(),
            2,
            "the stall sent a second request beside the load"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_cold_target_gets_one_long_request_then_the_normal_limits() {
        let t = FakeTranscriber::file([
            FakeStep::text("loaded", Duration::from_secs(40)),
            FakeStep::text("warm", Duration::from_millis(300)),
        ]);
        let up = upload(Arc::clone(&t), 90_000);
        assert_eq!(up.unit_deadline_ms(), 90_000);
        let r = up
            .run(
                SegmentAudio::new(vec![0; 16_000]),
                unit(up.unit_deadline_ms()),
            )
            .await;
        assert_eq!(r.result.map(|t| t.text), Ok("loaded".to_string()));
        assert_eq!(
            t.calls(),
            1,
            "no stall-triggered second request while loading"
        );
        assert_eq!(
            up.store.round_trip("openai:any").samples,
            0,
            "a load is not a round trip"
        );
        assert_eq!(
            up.unit_deadline_ms(),
            6000,
            "warm: the latency contract is back"
        );
        let r = up.run(SegmentAudio::new(vec![0; 16_000]), unit(6000)).await;
        assert_eq!(r.result.map(|t| t.text), Ok("warm".to_string()));
    }

    fn seg(start: u64, pcm_len: usize, value: i16, zeros: u32) -> Segment {
        Segment {
            first_speech_sample: start,
            last_speech_end_sample: start + pcm_len as u64,
            audio_start_sample: start,
            audio_end_sample: start + pcm_len as u64,
            pcm: vec![value; pcm_len],
            trailing_zero_samples: zeros,
            voiced_ms: 0,
            speech_span_ms: 0,
            mean_probability: 0.9,
            short: false,
            cut: CutReason::Pause,
        }
    }

    #[test]
    fn joined_audio_puts_silence_between_segments_capped_at_600_ms() {
        let a = seg(0, 100, 1, 8000);
        let b = seg(100 + 20_000, 50, 2, 8000);
        let j = join_segments(&[a, b], 600);
        assert_eq!(j.len(), 100 + 9600 + 50 + 8000);
        assert_eq!(&j[..100], &[1; 100][..]);
        assert!(j[100..9700].iter().all(|s| *s == 0));
        assert_eq!(&j[9700..9750], &[2; 50][..]);
    }

    #[test]
    fn overlapping_pre_roll_is_taken_from_where_the_first_segment_ended() {
        let a = seg(0, 100, 1, 0);
        let b = seg(80, 50, 2, 10);
        let j = join_segments(&[a, b], 600);
        assert_eq!(j.len(), 100 + 30 + 10);
    }

    #[test]
    fn a_single_segment_is_its_audio_then_its_zeros() {
        let j = join_segments(&[seg(0, 10, 3, 5)], 600);
        assert_eq!(j, vec![3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 0, 0, 0, 0, 0]);
    }
}
