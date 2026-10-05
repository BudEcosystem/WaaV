//! Upload units: the seam the engine calls once per unit, and the joining of held segments.
//!
//! The engine keeps the units and turns themselves (it is their single owner); this module holds
//! what is independent of the engine task: the [`SegmentUpload`] seam, its production form over the
//! attempt loop and the latency store, and the join rule.

use std::sync::Arc;

use tokio::time::Instant;

use crate::limits::LatencyStore;
use crate::segmenter::Segment;
use crate::transcriber::SegmentContext;
use crate::transcriber::SegmentAudio;
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
    /// May a unit that is not the turn's last start now? Never waits.
    fn try_speculative(&self) -> bool;
    async fn prewarm(&self, connections: usize);
    /// The vendor's shortest accepted upload.
    fn min_audio_ms(&self) -> u32;
    /// End of speech to the release of the unit's text.
    fn record_end_to_final(&self, _ms: u32) {}
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
}

#[async_trait::async_trait]
impl SegmentUpload for AttemptsUpload {
    async fn run(&self, audio: SegmentAudio, req: UnitUpload) -> UploadResolution {
        let deadlines = self.store.deadlines(
            &self.key,
            audio.audio_ms(),
            self.deadline_ms,
            self.row_request_timeout_ms,
            self.low_latency,
        );
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
            self.store.record_round_trip(&self.key, rt.as_millis() as u32);
        }
        if matches!(r.result, Err(crate::transcriber::attempts::UnitFailure::TimedOut)) {
            self.store.record_timeout(&self.key);
        }
        r
    }

    fn deadline_ms(&self) -> u32 {
        self.deadline_ms
    }

    fn try_speculative(&self) -> bool {
        self.limiter.has_headroom()
    }

    async fn prewarm(&self, connections: usize) {
        self.attempts.transcriber.prewarm(connections).await;
    }

    fn min_audio_ms(&self) -> u32 {
        self.attempts.info().min_audio_ms.unwrap_or(0)
    }

    fn record_end_to_final(&self, ms: u32) {
        self.store.record_end_to_final(&self.key, ms);
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
                    let gap = (s.audio_start_sample - prev_end).min(ms_to_samples(gap_cap_ms as u64));
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
    use crate::types::CutReason;

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
