//! The segmenter: a pure state machine over 32 ms frames and their speech probabilities.
//!
//! It confirms speech, opens a segment that starts with pre-roll (audio from before the first
//! speech frame), cuts at a pause, and splits very long speech. It reads no clock: time is the
//! count of samples received, so behaviour does not depend on delivery speed, and a lost frame
//! advances no time.
//!
//! Per frame the input is one of three kinds: *voiced* (probability at or above the entry
//! threshold and RMS at or above the energy floor), *quiet* (probability under the exit threshold,
//! or RMS under the floor), or *middle*.
//!
//! | State | voiced | middle | quiet |
//! | --- | --- | --- | --- |
//! | Idle | Onset | stay | stay |
//! | Onset | count; confirm at 224 ms | stay | gap; reject after 64 ms |
//! | Speech | append | append (cut as uncertain at 640 ms) | append; Hangover |
//! | Hangover | append; Speech | append (cut at the 224 ms pause) | append (cut at the pause) |

use std::collections::VecDeque;

use crate::audio::rms;
use crate::profile::SegmentProfile;
use crate::types::{CutReason, FRAME_MS, FRAME_SAMPLES, ms_to_frames_ceil, ms_to_samples};

/// How much recent audio is kept for pre-roll, commits and the audio end-of-turn model.
pub const RING_FRAMES: usize = 250; // 8 s

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegPhase {
    Idle,
    Onset,
    Speech,
    Hangover,
}

/// One stretch of caller speech, ready to upload.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub first_speech_sample: u64,
    pub last_speech_end_sample: u64,
    pub audio_start_sample: u64,
    pub audio_end_sample: u64,
    /// Real audio only: pre-roll, speech and hangover.
    pub pcm: Vec<i16>,
    /// Zeros appended at upload: removes invented words at the start and lost words at the end.
    pub trailing_zero_samples: u32,
    pub voiced_ms: u32,
    /// First speech sample to the end of the last speech frame.
    pub speech_span_ms: u32,
    pub mean_probability: f32,
    /// Under one second of speech: filtered more strictly.
    pub short: bool,
    pub cut: CutReason,
}

impl Segment {
    /// The audio as uploaded: real audio then the zeros.
    pub fn padded_pcm(&self) -> Vec<i16> {
        let mut out = Vec::with_capacity(self.pcm.len() + self.trailing_zero_samples as usize);
        out.extend_from_slice(&self.pcm);
        out.resize(self.pcm.len() + self.trailing_zero_samples as usize, 0);
        out
    }

    /// Real audio plus padding, in ms.
    pub fn audio_ms(&self) -> u32 {
        ((self.pcm.len() as u64 + self.trailing_zero_samples as u64) * 1000 / 16_000) as u32
    }
}

/// What a cut produced.
#[derive(Debug, Clone, PartialEq)]
pub enum CutOutcome {
    Ready(Segment),
    /// Under the minimum speech span; never uploaded.
    TooShort,
    /// The continuation after a split held no speech; dropped silently.
    Nothing,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SegmenterEvent {
    /// Speech confirmed, and again at the progress cadence while the run continues.
    Started {
        first_speech_sample: u64,
        sustained_ms: u32,
    },
    /// A length split: the caller has not stopped, so no stop is reported.
    Split(Segment),
    /// A pause, a commit, an input stall or the turn limit cut the open segment.
    Cut {
        reason: CutReason,
        first_speech_sample: u64,
        last_speech_end_sample: u64,
        voiced_ms: u32,
        outcome: CutOutcome,
    },
    /// An onset that never confirmed.
    OnsetRejected,
}

#[derive(Debug, Clone)]
struct RingFrame {
    index: u64,
    pcm: Vec<i16>,
    prob: f32,
    voiced: bool,
    loud: bool,
}

#[derive(Debug, Clone, Copy)]
struct FrameMeta {
    index: u64,
    prob: f32,
    voiced: bool,
}

#[derive(Debug)]
struct OpenSegment {
    audio_start: u64,
    pcm: Vec<i16>,
    frames: Vec<FrameMeta>,
    first_speech: Option<u64>,
    last_speech_end: Option<u64>,
}

impl OpenSegment {
    fn audio_ms(&self) -> u32 {
        (self.pcm.len() as u64 * 1000 / 16_000) as u32
    }

    fn voiced_frames(&self) -> u32 {
        self.frames.iter().filter(|f| f.voiced).count() as u32
    }

    fn mean_probability(&self) -> f32 {
        if self.frames.is_empty() {
            return 0.0;
        }
        self.frames.iter().map(|f| f.prob).sum::<f32>() / self.frames.len() as f32
    }

    /// The part of this segment from `start` (a sample) on, as a new open segment.
    fn tail_from(&self, start: u64) -> OpenSegment {
        let offset = (start.saturating_sub(self.audio_start) as usize).min(self.pcm.len());
        let frames: Vec<FrameMeta> = self
            .frames
            .iter()
            .copied()
            .filter(|f| f.index * FRAME_SAMPLES as u64 >= start)
            .collect();
        let first_speech = frames
            .iter()
            .find(|f| f.voiced)
            .map(|f| f.index * FRAME_SAMPLES as u64);
        let last_speech_end = frames
            .iter()
            .rev()
            .find(|f| f.voiced)
            .map(|f| (f.index + 1) * FRAME_SAMPLES as u64);
        OpenSegment {
            audio_start: start.max(self.audio_start),
            pcm: self.pcm[offset..].to_vec(),
            frames,
            first_speech,
            last_speech_end,
        }
    }

    /// This segment truncated at `end` (a sample).
    fn head_until(&self, end: u64) -> OpenSegment {
        let len = (end.saturating_sub(self.audio_start) as usize).min(self.pcm.len());
        let frames: Vec<FrameMeta> = self
            .frames
            .iter()
            .copied()
            .filter(|f| (f.index + 1) * FRAME_SAMPLES as u64 <= end)
            .collect();
        let last_speech_end = frames
            .iter()
            .rev()
            .find(|f| f.voiced)
            .map(|f| (f.index + 1) * FRAME_SAMPLES as u64);
        OpenSegment {
            audio_start: self.audio_start,
            pcm: self.pcm[..len].to_vec(),
            frames,
            first_speech: self.first_speech,
            last_speech_end,
        }
    }
}

/// The segmenter. See the module documentation for the state table.
pub struct Segmenter {
    p: SegmentProfile,
    enter_threshold: f32,
    exit_threshold: f32,
    frame_index: u64,
    ring: VecDeque<RingFrame>,
    phase: SegPhase,
    onset_first_frame: u64,
    onset_voiced: u32,
    onset_gap: u32,
    seg: Option<OpenSegment>,
    run_first_speech: u64,
    run_voiced_frames: u32,
    next_progress_ms: u32,
    silence_frames: u32,
    last_voiced_end: Option<u64>,
    prev_segment_speech_end: u64,
    last_cut_sample: u64,
}

impl std::fmt::Debug for Segmenter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Segmenter")
            .field("phase", &self.phase)
            .field("frame_index", &self.frame_index)
            .finish()
    }
}

impl Segmenter {
    pub fn new(profile: &SegmentProfile) -> Self {
        Self {
            enter_threshold: profile.enter_threshold,
            exit_threshold: profile.exit_threshold,
            p: profile.clone(),
            frame_index: 0,
            ring: VecDeque::with_capacity(RING_FRAMES),
            phase: SegPhase::Idle,
            onset_first_frame: 0,
            onset_voiced: 0,
            onset_gap: 0,
            seg: None,
            run_first_speech: 0,
            run_voiced_frames: 0,
            next_progress_ms: 0,
            silence_frames: 0,
            last_voiced_end: None,
            prev_segment_speech_end: 0,
            last_cut_sample: 0,
        }
    }

    pub fn phase(&self) -> SegPhase {
        self.phase
    }

    /// The sample clock: samples received so far.
    pub fn now_sample(&self) -> u64 {
        self.frame_index * FRAME_SAMPLES as u64
    }

    /// Samples since the end of the last voiced frame (since the start when there was none).
    pub fn silence_samples(&self) -> u64 {
        self.now_sample() - self.last_voiced_end.unwrap_or(0)
    }

    /// The end of the last voiced frame, if any.
    pub fn last_voiced_end(&self) -> Option<u64> {
        self.last_voiced_end
    }

    /// Whether any audio arrived since the last cut or commit.
    pub fn audio_since_last_cut(&self) -> bool {
        self.now_sample() > self.last_cut_sample
    }

    pub fn thresholds(&self) -> (f32, f32) {
        (self.enter_threshold, self.exit_threshold)
    }

    /// Recent audio from `from_sample` to now, at most the ring's 8 s.
    pub fn tail(&self, from_sample: u64) -> Vec<i16> {
        let mut out = Vec::new();
        for f in &self.ring {
            let start = f.index * FRAME_SAMPLES as u64;
            let end = start + FRAME_SAMPLES as u64;
            if end <= from_sample {
                continue;
            }
            let skip = from_sample.saturating_sub(start) as usize;
            out.extend_from_slice(&f.pcm[skip.min(FRAME_SAMPLES)..]);
        }
        out
    }

    /// Raise the entry and exit thresholds after noise was uploaded as speech. Returns the new
    /// entry threshold.
    pub fn raise_thresholds(&mut self, step: f32, ceiling: f32) -> f32 {
        self.enter_threshold = (self.enter_threshold + step).min(ceiling);
        self.exit_threshold = (self.exit_threshold + step).min(self.enter_threshold - 0.05);
        self.enter_threshold
    }

    /// Drop the open segment without uploading it (noise escalation) and return to Idle.
    /// Returns the voiced time that was dropped.
    pub fn abandon_open_segment(&mut self) -> u32 {
        let dropped = self.seg.take().map(|s| s.voiced_frames() * FRAME_MS).unwrap_or(0);
        if let Some(end) = self.last_voiced_end {
            self.prev_segment_speech_end = end;
        }
        self.last_cut_sample = self.now_sample();
        self.to_idle();
        dropped
    }

    /// One frame of 16 kHz PCM and its speech probability.
    pub fn push_frame(&mut self, frame: &[i16], probability: f32, out: &mut Vec<SegmenterEvent>) {
        debug_assert_eq!(frame.len(), FRAME_SAMPLES);
        let level = rms(frame);
        let loud = level >= self.p.energy_floor_rms;
        let voiced = probability >= self.enter_threshold && loud;
        let quiet = probability < self.exit_threshold || !loud;
        let index = self.frame_index;
        let start = index * FRAME_SAMPLES as u64;
        let end = start + FRAME_SAMPLES as u64;

        if self.ring.len() == RING_FRAMES {
            self.ring.pop_front();
        }
        self.ring.push_back(RingFrame {
            index,
            pcm: frame.to_vec(),
            prob: probability,
            voiced,
            loud,
        });
        self.frame_index += 1;
        if voiced {
            self.last_voiced_end = Some(end);
        }

        match self.phase {
            SegPhase::Idle => {
                if voiced {
                    self.phase = SegPhase::Onset;
                    self.onset_first_frame = index;
                    self.onset_voiced = 1;
                    self.onset_gap = 0;
                    self.maybe_confirm(end, out);
                }
            }
            SegPhase::Onset => {
                if voiced {
                    self.onset_voiced += 1;
                    self.onset_gap = 0;
                } else if quiet {
                    self.onset_gap += 1;
                }
                let frames_in_onset = (index - self.onset_first_frame + 1) as u32;
                if voiced && self.maybe_confirm(end, out) {
                    // confirmed
                } else if self.onset_gap * FRAME_MS > self.p.onset_max_gap_ms
                    || frames_in_onset >= ms_to_frames_ceil(self.p.onset_window_ms)
                {
                    self.phase = SegPhase::Idle;
                    out.push(SegmenterEvent::OnsetRejected);
                }
            }
            SegPhase::Speech | SegPhase::Hangover => {
                self.append(index, frame, probability, voiced);
                if voiced {
                    self.phase = SegPhase::Speech;
                    self.silence_frames = 0;
                    self.run_voiced_frames += 1;
                    let seg = self.seg.as_mut().expect("open segment in speech");
                    seg.last_speech_end = Some(end);
                    if seg.first_speech.is_none() {
                        seg.first_speech = Some(start);
                    }
                    let sustained = self.run_voiced_frames * FRAME_MS;
                    if sustained >= self.next_progress_ms {
                        out.push(SegmenterEvent::Started {
                            first_speech_sample: self.run_first_speech,
                            sustained_ms: sustained,
                        });
                        self.next_progress_ms += 128;
                    }
                } else {
                    self.silence_frames += 1;
                    if quiet {
                        self.phase = SegPhase::Hangover;
                    }
                    let silence_ms = self.silence_frames * FRAME_MS;
                    if self.phase == SegPhase::Hangover
                        && silence_ms >= ms_to_frames_ceil(self.p.cut_silence_ms) * FRAME_MS
                    {
                        self.cut(CutReason::Pause, true, out);
                        return;
                    }
                    if self.phase == SegPhase::Speech
                        && silence_ms >= ms_to_frames_ceil(self.p.max_uncertain_ms) * FRAME_MS
                    {
                        self.cut(CutReason::Uncertain, true, out);
                        return;
                    }
                }
                self.check_length(out);
            }
        }
    }

    /// Cut now for an input stall or the turn limit: an open segment is cut with the minimum
    /// speech rule; an onset is abandoned.
    pub fn flush(&mut self, reason: CutReason, out: &mut Vec<SegmenterEvent>) {
        match self.phase {
            SegPhase::Speech | SegPhase::Hangover => self.cut(reason, true, out),
            SegPhase::Onset => {
                self.phase = SegPhase::Idle;
                out.push(SegmenterEvent::OnsetRejected);
            }
            SegPhase::Idle => {}
        }
    }

    /// A client commit. An open segment is cut at once whatever its length. Otherwise the audio
    /// since the last cut becomes a segment when it is at least `min_audio_ms` long and has at
    /// least three frames at or above the energy floor; it is reported as an ordinary start and
    /// cut. Silence is never uploaded.
    pub fn commit(&mut self, min_audio_ms: u32, out: &mut Vec<SegmenterEvent>) {
        match self.phase {
            SegPhase::Speech | SegPhase::Hangover => {
                self.cut(CutReason::Commit, false, out);
                return;
            }
            SegPhase::Onset | SegPhase::Idle => {}
        }
        let from = self.last_cut_sample;
        let frames: Vec<&RingFrame> = self
            .ring
            .iter()
            .filter(|f| f.index * FRAME_SAMPLES as u64 >= from)
            .collect();
        let loud: Vec<&&RingFrame> = frames.iter().filter(|f| f.loud).collect();
        let audio_ms = frames.len() as u32 * FRAME_MS;
        self.last_cut_sample = self.now_sample();
        self.phase = SegPhase::Idle;
        if loud.len() < 3 || audio_ms < min_audio_ms {
            return;
        }
        let first_speech = loud[0].index * FRAME_SAMPLES as u64;
        let last_end = (loud[loud.len() - 1].index + 1) * FRAME_SAMPLES as u64;
        let audio_start = frames[0].index * FRAME_SAMPLES as u64;
        let pcm: Vec<i16> = frames.iter().flat_map(|f| f.pcm.iter().copied()).collect();
        let voiced_ms = loud.len() as u32 * FRAME_MS;
        let mean = frames.iter().map(|f| f.prob).sum::<f32>() / frames.len() as f32;
        let audio_end = self.now_sample();
        let segment = Segment {
            first_speech_sample: first_speech,
            last_speech_end_sample: last_end,
            audio_start_sample: audio_start,
            audio_end_sample: audio_end,
            pcm,
            trailing_zero_samples: self.trailing_zeros(audio_end - last_end),
            voiced_ms,
            speech_span_ms: ((last_end - first_speech) * 1000 / 16_000) as u32,
            mean_probability: mean,
            short: (last_end - first_speech) < ms_to_samples(self.p.short_segment_ms as u64),
            cut: CutReason::Commit,
        };
        self.prev_segment_speech_end = last_end;
        out.push(SegmenterEvent::Started {
            first_speech_sample: first_speech,
            sustained_ms: voiced_ms,
        });
        out.push(SegmenterEvent::Cut {
            reason: CutReason::Commit,
            first_speech_sample: first_speech,
            last_speech_end_sample: last_end,
            voiced_ms,
            outcome: CutOutcome::Ready(segment),
        });
    }

    fn maybe_confirm(&mut self, frame_end: u64, out: &mut Vec<SegmenterEvent>) -> bool {
        if self.onset_voiced < ms_to_frames_ceil(self.p.start_confirm_ms) {
            return false;
        }
        let first_speech = self.onset_first_frame * FRAME_SAMPLES as u64;
        let ring_start = self
            .ring
            .front()
            .map(|f| f.index * FRAME_SAMPLES as u64)
            .unwrap_or(0);
        let audio_start = first_speech
            .saturating_sub(ms_to_samples(self.p.pre_roll_ms as u64))
            .max(self.prev_segment_speech_end)
            .max(ring_start);
        let pcm = self.tail(audio_start);
        let frames = self
            .ring
            .iter()
            .filter(|f| (f.index + 1) * FRAME_SAMPLES as u64 > audio_start)
            .map(|f| FrameMeta {
                index: f.index,
                prob: f.prob,
                voiced: f.voiced && f.index >= self.onset_first_frame,
            })
            .collect();
        self.seg = Some(OpenSegment {
            audio_start,
            pcm,
            frames,
            first_speech: Some(first_speech),
            last_speech_end: Some(frame_end),
        });
        self.phase = SegPhase::Speech;
        self.silence_frames = 0;
        self.run_first_speech = first_speech;
        self.run_voiced_frames = self.onset_voiced;
        let sustained = self.run_voiced_frames * FRAME_MS;
        out.push(SegmenterEvent::Started {
            first_speech_sample: first_speech,
            sustained_ms: sustained,
        });
        self.next_progress_ms = 384;
        true
    }

    fn append(&mut self, index: u64, frame: &[i16], prob: f32, voiced: bool) {
        if let Some(seg) = self.seg.as_mut() {
            seg.pcm.extend_from_slice(frame);
            seg.frames.push(FrameMeta {
                index,
                prob,
                voiced,
            });
        }
    }

    fn trailing_zeros(&self, hangover_samples: u64) -> u32 {
        let cap = ms_to_samples(self.p.trailing_cap_ms as u64);
        let want = ms_to_samples(self.p.trailing_silence_ms as u64);
        want.min(cap.saturating_sub(hangover_samples)) as u32
    }

    fn finish(&self, open: &OpenSegment, audio_end: u64, cut: CutReason) -> Option<Segment> {
        let first = open.first_speech?;
        let last = open.last_speech_end?;
        let span = last.saturating_sub(first);
        Some(Segment {
            first_speech_sample: first,
            last_speech_end_sample: last,
            audio_start_sample: open.audio_start,
            audio_end_sample: audio_end,
            pcm: open.pcm.clone(),
            trailing_zero_samples: self.trailing_zeros(audio_end.saturating_sub(last)),
            voiced_ms: open.voiced_frames() * FRAME_MS,
            speech_span_ms: (span * 1000 / 16_000) as u32,
            mean_probability: open.mean_probability(),
            short: span < ms_to_samples(self.p.short_segment_ms as u64),
            cut,
        })
    }

    fn cut(&mut self, reason: CutReason, min_rule: bool, out: &mut Vec<SegmenterEvent>) {
        let open = self.seg.take().expect("cut with an open segment");
        let audio_end = self.now_sample();
        let last_speech_end = self.last_voiced_end.unwrap_or(audio_end);
        let voiced_ms = self.run_voiced_frames * FRAME_MS;
        let first_speech = self.run_first_speech;
        let outcome = match self.finish(&open, audio_end, reason) {
            None => CutOutcome::Nothing,
            Some(seg)
                if min_rule
                    && seg.speech_span_ms
                        < ms_to_frames_ceil(self.p.min_voiced_ms) * FRAME_MS =>
            {
                CutOutcome::TooShort
            }
            Some(seg) => CutOutcome::Ready(seg),
        };
        self.prev_segment_speech_end = last_speech_end;
        self.last_cut_sample = audio_end;
        out.push(SegmenterEvent::Cut {
            reason,
            first_speech_sample: first_speech,
            last_speech_end_sample: last_speech_end,
            voiced_ms,
            outcome,
        });
        self.to_idle();
    }

    fn to_idle(&mut self) {
        self.seg = None;
        self.phase = SegPhase::Idle;
        self.silence_frames = 0;
        self.run_voiced_frames = 0;
        self.onset_voiced = 0;
        self.onset_gap = 0;
    }

    fn check_length(&mut self, out: &mut Vec<SegmenterEvent>) {
        let Some(seg) = self.seg.as_ref() else {
            return;
        };
        let now = self.now_sample();
        let audio_ms = seg.audio_ms();
        if audio_ms >= self.p.soft_max_segment_ms()
            && self.silence_frames >= ms_to_frames_ceil(self.p.split_pause_ms)
            && seg.last_speech_end.is_some()
        {
            // Split at the micro-pause: the pause audio goes to both sides.
            let split_at = seg.last_speech_end.expect("checked");
            let first = seg.head_until(now);
            let rest = seg.tail_from(split_at);
            if let Some(done) = self.finish(&first, now, CutReason::SoftSplit) {
                out.push(SegmenterEvent::Split(done));
            }
            self.seg = Some(rest);
            return;
        }
        if audio_ms >= self.p.max_segment_ms {
            self.hard_split(out);
        }
    }

    fn hard_split(&mut self, out: &mut Vec<SegmenterEvent>) {
        let seg = self.seg.take().expect("open segment");
        let now = self.now_sample();
        let half = seg.frames.len() / 2;
        let second = &seg.frames[half..];
        // The longest run of non-voiced frames in the second half.
        let mut best: Option<(usize, usize)> = None;
        let mut run_start: Option<usize> = None;
        for (i, f) in second.iter().enumerate() {
            if !f.voiced {
                let s = *run_start.get_or_insert(i);
                let len = i - s + 1;
                if best.is_none_or(|(_, l)| len > l) {
                    best = Some((s, len));
                }
            } else {
                run_start = None;
            }
        }
        let (first_end, rest_start, reason) = match best {
            Some((s, len)) => {
                let run_first = second[s].index * FRAME_SAMPLES as u64;
                let run_end = (second[s + len - 1].index + 1) * FRAME_SAMPLES as u64;
                (run_end, run_first, CutReason::SoftSplit)
            }
            None => {
                let lowest = second
                    .iter()
                    .enumerate()
                    .min_by(|a, b| a.1.prob.total_cmp(&b.1.prob).then(b.0.cmp(&a.0)))
                    .map(|(_, f)| f.index)
                    .unwrap_or(seg.frames.last().map(|f| f.index).unwrap_or(0));
                let end = (lowest + 1) * FRAME_SAMPLES as u64;
                let overlap = ms_to_samples(self.p.hard_split_overlap_ms as u64);
                (end, end.saturating_sub(overlap).max(seg.audio_start), CutReason::HardSplit)
            }
        };
        let first = seg.head_until(first_end);
        let rest = seg.tail_from(rest_start);
        if let Some(done) = self.finish(&first, first_end, reason) {
            out.push(SegmenterEvent::Split(done));
        }
        let _ = now;
        self.seg = Some(rest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOUD: i16 = 3000; // RMS about 0.09
    const V: f32 = 0.9;
    const M: f32 = 0.45;
    const Q: f32 = 0.05;

    struct Run {
        seg: Segmenter,
        events: Vec<SegmenterEvent>,
    }

    impl Run {
        fn new() -> Self {
            Self::with(SegmentProfile::for_tests())
        }
        fn with(p: SegmentProfile) -> Self {
            Self {
                seg: Segmenter::new(&p),
                events: Vec::new(),
            }
        }
        fn frames(&mut self, n: usize, prob: f32) -> &mut Self {
            for _ in 0..n {
                let level = if prob <= Q { 0 } else { LOUD };
                let frame: Vec<i16> = (0..FRAME_SAMPLES)
                    .map(|i| if i % 2 == 0 { level } else { -level })
                    .collect();
                self.seg.push_frame(&frame, prob, &mut self.events);
            }
            self
        }
        fn v(&mut self, n: usize) -> &mut Self {
            self.frames(n, V)
        }
        fn q(&mut self, n: usize) -> &mut Self {
            self.frames(n, Q)
        }
        fn m(&mut self, n: usize) -> &mut Self {
            self.frames(n, M)
        }
        fn starts(&self) -> Vec<(u64, u32)> {
            self.events
                .iter()
                .filter_map(|e| match e {
                    SegmenterEvent::Started {
                        first_speech_sample,
                        sustained_ms,
                    } => Some((*first_speech_sample, *sustained_ms)),
                    _ => None,
                })
                .collect()
        }
        fn cuts(&self) -> Vec<(CutReason, u64, CutOutcome)> {
            self.events
                .iter()
                .filter_map(|e| match e {
                    SegmenterEvent::Cut {
                        reason,
                        last_speech_end_sample,
                        outcome,
                        ..
                    } => Some((*reason, *last_speech_end_sample, outcome.clone())),
                    _ => None,
                })
                .collect()
        }
        fn segments(&self) -> Vec<Segment> {
            self.events
                .iter()
                .filter_map(|e| match e {
                    SegmenterEvent::Split(s) => Some(s.clone()),
                    SegmenterEvent::Cut {
                        outcome: CutOutcome::Ready(s),
                        ..
                    } => Some(s.clone()),
                    _ => None,
                })
                .collect()
        }
    }

    #[test]
    fn speech_is_confirmed_after_seven_voiced_frames_and_reports_its_first_sample() {
        let mut r = Run::new();
        r.q(10).v(6);
        assert!(r.starts().is_empty());
        r.v(1);
        assert_eq!(r.starts(), vec![(5120, 224)]);
        assert_eq!(r.seg.phase(), SegPhase::Speech);
    }

    #[test]
    fn one_soft_frame_inside_an_onset_does_not_restart_confirmation() {
        let mut r = Run::new();
        r.q(5).v(3).q(2).v(4);
        assert_eq!(r.starts(), vec![(5 * 512, 224)]);
    }

    #[test]
    fn three_quiet_frames_reject_an_onset() {
        let mut r = Run::new();
        r.q(5).v(3).q(3);
        assert!(r.events.contains(&SegmenterEvent::OnsetRejected));
        assert_eq!(r.seg.phase(), SegPhase::Idle);
    }

    #[test]
    fn an_onset_held_by_middle_frames_is_rejected_after_its_window() {
        let mut r = Run::new();
        r.q(2).v(1).m(12);
        assert!(r.events.is_empty());
        r.m(1);
        assert_eq!(r.events, vec![SegmenterEvent::OnsetRejected]);
    }

    #[test]
    fn pre_roll_is_measured_back_from_the_first_speech_frame() {
        let mut r = Run::new();
        r.q(30).v(20).q(7);
        let segs = r.segments();
        assert_eq!(segs.len(), 1);
        let s = &segs[0];
        assert_eq!(s.first_speech_sample, 30 * 512);
        assert_eq!(s.audio_start_sample, 30 * 512 - 6400, "400 ms before the first speech frame");
    }

    #[test]
    fn pre_roll_never_reaches_back_into_the_previous_segment() {
        let mut r = Run::new();
        r.q(30).v(20).q(7).q(2).v(20).q(7);
        let segs = r.segments();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[1].audio_start_sample, segs[0].last_speech_end_sample);
    }

    #[test]
    fn a_pause_of_seven_frames_cuts_and_reports_the_last_speech_end() {
        let mut r = Run::new();
        r.q(30).v(20).q(6);
        assert!(r.cuts().is_empty(), "six quiet frames are not yet a pause");
        r.q(1);
        let cuts = r.cuts();
        assert_eq!(cuts.len(), 1);
        assert_eq!(cuts[0].0, CutReason::Pause);
        assert_eq!(cuts[0].1, 50 * 512);
        assert_eq!(r.seg.phase(), SegPhase::Idle);
    }

    #[test]
    fn uploaded_audio_ends_with_the_hangover_then_the_configured_zeros() {
        let mut r = Run::new();
        r.q(30).v(20).q(7);
        let s = &r.segments()[0];
        assert_eq!(s.audio_end_sample, 57 * 512, "real audio runs to the cut");
        assert_eq!(s.pcm.len() as u64, s.audio_end_sample - s.audio_start_sample);
        assert!(s.pcm[s.pcm.len() - 7 * 512..].iter().all(|x| *x == 0), "the hangover is real audio");
        assert_eq!(s.trailing_zero_samples, 8000, "500 ms of zeros");
        let padded = s.padded_pcm();
        assert_eq!(padded.len(), s.pcm.len() + 8000);
        assert_eq!(s.audio_ms(), ((s.pcm.len() + 8000) / 16) as u32);
    }

    #[test]
    fn hangover_plus_zeros_never_exceed_one_second() {
        let mut p = SegmentProfile::for_tests();
        p.trailing_silence_ms = 900;
        let mut r = Run::with(p);
        r.q(30).v(20).q(7);
        let s = &r.segments()[0];
        assert_eq!(s.trailing_zero_samples as u64, 16_000 - 7 * 512);
    }

    #[test]
    fn a_segment_under_the_minimum_speech_span_is_discarded() {
        let mut r = Run::new();
        r.q(10).v(7).q(7);
        assert_eq!(r.cuts()[0].2, CutOutcome::TooShort, "224 ms is under 256 ms");
        let mut r = Run::new();
        r.q(10).v(8).q(7);
        assert!(matches!(r.cuts()[0].2, CutOutcome::Ready(_)), "256 ms is enough");
    }

    #[test]
    fn middle_frames_keep_the_segment_open_until_the_uncertain_bound() {
        let mut r = Run::new();
        r.q(30).v(10).m(19);
        assert!(r.cuts().is_empty());
        r.m(1);
        assert_eq!(r.cuts()[0].0, CutReason::Uncertain);
    }

    #[test]
    fn after_a_quiet_frame_a_noise_floor_between_thresholds_still_cuts_at_the_pause() {
        let mut r = Run::new();
        r.q(30).v(10).q(1).m(5);
        assert!(r.cuts().is_empty());
        r.m(1);
        assert_eq!(r.cuts()[0].0, CutReason::Pause);
    }

    #[test]
    fn progress_is_reported_at_confirmation_at_384_ms_then_every_128_ms() {
        let mut r = Run::new();
        r.q(3).v(28);
        let sustained: Vec<u32> = r.starts().iter().map(|s| s.1).collect();
        assert_eq!(sustained, vec![224, 384, 512, 640, 768, 896]);
        assert!(r.starts().iter().all(|s| s.0 == 3 * 512));
    }

    #[test]
    fn a_split_shares_the_pause_and_no_speech_sample_is_lost_or_duplicated() {
        let mut r = Run::new();
        r.q(30);
        for _ in 0..20 {
            r.v(60).q(3); // about two seconds of speech, then a 96 ms dip
        }
        r.q(7);
        let segs = r.segments();
        assert!(segs.len() >= 2, "{}", segs.len());
        assert_eq!(segs[0].cut, CutReason::SoftSplit);
        assert!(segs[0].audio_ms() <= 25_000 + 1_000);
        for w in segs.windows(2) {
            // The second starts exactly where the first's speech ended: the shared pause is in both.
            assert_eq!(w[1].audio_start_sample, w[0].last_speech_end_sample);
            assert!(w[0].audio_end_sample > w[1].audio_start_sample);
            assert!(w[1].first_speech_sample >= w[0].last_speech_end_sample);
        }
        // No Stopped (cut) for the split: one cut, at the end.
        assert_eq!(r.cuts().len(), 1);
        let total_voiced: u32 = segs.iter().map(|s| s.voiced_ms).sum();
        assert_eq!(total_voiced, 20 * 60 * 32);
    }

    #[test]
    fn speech_with_no_pause_is_split_hard_with_an_overlap_at_the_lowest_frame() {
        let mut r = Run::new();
        r.q(30).v(600).frames(1, 0.6).v(400);
        let segs = r.segments();
        assert_eq!(segs.len(), 1, "the continuation is still open");
        let first = &segs[0];
        assert_eq!(first.cut, CutReason::HardSplit);
        let low_frame_end = (30 + 600 + 1) as u64 * 512;
        assert_eq!(first.audio_end_sample, low_frame_end);
        r.q(7);
        let segs = r.segments();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].audio_end_sample - segs[1].audio_start_sample, 5120, "320 ms overlap");
    }

    #[test]
    fn a_commit_cuts_the_open_segment_whatever_its_length() {
        let mut r = Run::new();
        r.q(5).v(7);
        r.seg.commit(100, &mut r.events);
        let cuts = r.cuts();
        assert_eq!(cuts[0].0, CutReason::Commit);
        assert!(matches!(cuts[0].2, CutOutcome::Ready(_)));
    }

    #[test]
    fn a_commit_after_unconfirmed_loud_audio_uploads_it_as_a_turn() {
        let mut r = Run::new();
        r.q(5).v(4);
        r.seg.commit(100, &mut r.events);
        assert_eq!(r.starts().len(), 1);
        let segs = r.segments();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].first_speech_sample, 5 * 512);
        assert_eq!(segs[0].cut, CutReason::Commit);
    }

    #[test]
    fn a_commit_after_only_silence_uploads_nothing() {
        let mut r = Run::new();
        r.q(20);
        r.seg.commit(100, &mut r.events);
        assert!(r.events.is_empty());
        assert!(!r.seg.audio_since_last_cut());
    }

    #[test]
    fn raised_thresholds_turn_borderline_speech_into_quiet() {
        let mut r = Run::new();
        assert!((r.seg.raise_thresholds(0.1, 0.8) - 0.6).abs() < 1e-6);
        assert!((r.seg.raise_thresholds(0.1, 0.8) - 0.7).abs() < 1e-6);
        r.q(5).frames(20, 0.65);
        assert!(r.starts().is_empty());
        assert!((r.seg.raise_thresholds(0.5, 0.8) - 0.8).abs() < 1e-6, "capped at the ceiling");
    }

    #[test]
    fn an_input_stall_flush_cuts_an_open_segment_and_abandons_an_onset() {
        let mut r = Run::new();
        r.q(5).v(10);
        r.seg.flush(CutReason::InputStall, &mut r.events);
        assert_eq!(r.cuts()[0].0, CutReason::InputStall);
        let mut r = Run::new();
        r.q(5).v(3);
        r.seg.flush(CutReason::InputStall, &mut r.events);
        assert_eq!(r.events, vec![SegmenterEvent::OnsetRejected]);
    }

    #[test]
    fn the_tail_returns_recent_audio_from_a_sample() {
        let mut r = Run::new();
        r.q(10).v(10);
        assert_eq!(r.seg.tail(15 * 512).len(), 5 * 512);
        assert_eq!(r.seg.tail(15 * 512 + 100).len(), 5 * 512 - 100);
        assert_eq!(r.seg.silence_samples(), 0);
        r.q(3);
        assert_eq!(r.seg.silence_samples(), 3 * 512);
    }

    #[test]
    fn abandoning_drops_the_open_segment_and_returns_its_voiced_time() {
        let mut r = Run::new();
        r.q(5).v(10);
        assert_eq!(r.seg.abandon_open_segment(), 320);
        assert_eq!(r.seg.phase(), SegPhase::Idle);
        r.q(7);
        assert!(r.cuts().is_empty());
    }
}
