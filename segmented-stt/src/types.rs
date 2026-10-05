//! The values the engine reports: speech activity, upload outcomes, results, facts and notices.
//!
//! The engine reports facts, never customer sentences. The gateway's contract module writes every
//! string a client sees.

use std::sync::Arc;

/// The engine's internal sample rate. Every detector, segment and upload runs at 16 kHz mono.
pub const SAMPLE_RATE: u32 = 16_000;
/// One detector frame: 512 samples, the size the Silero model requires.
pub const FRAME_SAMPLES: usize = 512;
/// One frame in milliseconds.
pub const FRAME_MS: u32 = 32;

/// Milliseconds to samples at 16 kHz.
pub const fn ms_to_samples(ms: u64) -> u64 {
    ms * (SAMPLE_RATE as u64) / 1000
}

/// Samples at 16 kHz to milliseconds, rounded down.
pub const fn samples_to_ms(samples: u64) -> u64 {
    samples * 1000 / (SAMPLE_RATE as u64)
}

/// A duration in milliseconds rounded up to whole 32 ms frames, the granularity every
/// threshold is checked at.
pub const fn ms_to_frames_ceil(ms: u32) -> u32 {
    ms.div_ceil(FRAME_MS)
}

/// Which detector a session runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DetectorKind {
    /// The Silero neural detector.
    Silero,
    /// The loudness-based detector: builds without Silero, or where the operator allowed it.
    Energy,
    /// A scripted detector, in tests only.
    Scripted,
}

impl DetectorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Silero => "silero",
            Self::Energy => "energy",
            Self::Scripted => "scripted",
        }
    }
}

/// Why a session runs a worse detector than the build could provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectorFallback {
    /// The build has no Silero feature.
    FeatureNotBuilt,
    /// The Silero model could not be loaded and the operator allowed the energy detector.
    ModelUnavailable,
    /// The Silero detector failed at run time and the operator allowed the energy detector.
    RuntimeErrors,
}

impl DetectorFallback {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FeatureNotBuilt => "feature_not_built",
            Self::ModelUnavailable => "model_unavailable",
            Self::RuntimeErrors => "runtime_errors",
        }
    }
}

/// Why a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TurnCloseReason {
    /// The audio end-of-turn model said the caller finished.
    EndOfTurnModel,
    /// The text end-of-turn model read the turn's text as complete.
    TextModel,
    /// The silence rule: no model, or a session whose policy is silence only.
    SilenceThreshold,
    /// The silence ceiling.
    MaxEndpointing,
    /// The client stopped sending audio and the ceiling was reached on input-idle time.
    InputIdle,
    /// The client committed the turn (`audio_end`, a Realtime commit).
    Commit,
    /// The turn reached its longest allowed duration.
    MaxTurnDuration,
    /// Every unit of the turn resolved and none had text.
    NoText,
}

impl TurnCloseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EndOfTurnModel => "end_of_turn_model",
            Self::TextModel => "text_model",
            Self::SilenceThreshold => "silence_threshold",
            Self::MaxEndpointing => "max_endpointing",
            Self::InputIdle => "input_idle",
            Self::Commit => "commit",
            Self::MaxTurnDuration => "max_turn_duration",
            Self::NoText => "no_text",
        }
    }

    /// A sealing endpoint is not withdrawn by new speech.
    pub fn is_sealing(self) -> bool {
        matches!(self, Self::Commit | Self::MaxTurnDuration)
    }
}

/// What the detector heard, reported on the engine task before any transcript exists.
#[derive(Debug, Clone, PartialEq)]
pub enum SpeechActivity {
    /// Speech confirmed, and again at 384 ms of voiced time and every 128 ms after.
    /// `at_sample` is the first speech sample of the run, not the confirmation.
    Started {
        turn_id: u64,
        at_sample: u64,
        sustained_ms: u32,
        at_mono_ms: u64,
    },
    /// A pause, a commit or an input stall cut a segment. `at_sample` is the end of the last
    /// speech frame. `will_upload` is false when the segment was discarded or refused.
    Stopped {
        turn_id: u64,
        at_sample: u64,
        voiced_ms: u32,
        will_upload: bool,
        at_mono_ms: u64,
    },
    /// The endpoint is in force for the turn's latest pause; its uploads may still be running.
    /// A later `Started` of the same turn withdraws it.
    EndpointDecided {
        turn_id: u64,
        reason: TurnCloseReason,
        at_mono_ms: u64,
    },
    /// Every upload of the turn has resolved. `result_follows`: one end-of-turn result is about to
    /// be queued (the turn has text, or it was sealed by a commit).
    TurnClosed {
        turn_id: u64,
        had_text: bool,
        result_follows: bool,
        segments: u16,
        gaps: u16,
        lost_voiced_ms: u32,
        reason: TurnCloseReason,
        speech_end_mono_ms: u64,
    },
}

impl SpeechActivity {
    pub fn turn_id(&self) -> u64 {
        match self {
            Self::Started { turn_id, .. }
            | Self::Stopped { turn_id, .. }
            | Self::EndpointDecided { turn_id, .. }
            | Self::TurnClosed { turn_id, .. } => *turn_id,
        }
    }
}

/// What the admission hook is asked about one segment at its cut.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentMeta {
    pub turn_id: u64,
    pub seq: u32,
    pub voiced_ms: u32,
    pub first_speech_sample: u64,
    pub last_speech_end_sample: u64,
}

/// The admission answer for one segment. Unset hooks admit everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentAdmission {
    /// False: the speech is not input (it overlapped a greeting that may not be cut); nothing is
    /// uploaded and the segment resolves as filtered.
    pub is_input: bool,
    /// Evidence for the quality verdict: the agent was audible while the caller spoke.
    pub overlapped_agent_speech: bool,
    pub agent_spoke_since_previous_segment: bool,
}

impl SegmentAdmission {
    pub const ADMIT: Self = Self {
        is_input: true,
        overlapped_agent_speech: false,
        agent_spoke_since_previous_segment: false,
    };
}

/// What a flush (a client commit) did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlushOutcome {
    /// The sealed turn, or `None` when the commit did nothing (no audio since the last commit or
    /// turn final).
    pub turn_id: Option<u64>,
    pub will_upload: bool,
    /// True whenever a turn was sealed: that turn always gets exactly one final result.
    pub result_follows: bool,
}

/// Whether interim results are emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterimMode {
    /// One interim per returned segment, carrying the whole turn so far.
    PerSegment,
    /// None (the client set `interim_results: false`, or the row uploads once per turn).
    Off,
}

/// What a running engine knows about itself. Facts, not sentences.
#[derive(Debug, Clone, PartialEq)]
pub struct SttLiveFacts {
    pub detector: DetectorKind,
    pub detector_fallback: Option<DetectorFallback>,
    /// The wire rate the audio was resampled from, when it was not 16 kHz.
    pub resampled_from_hz: Option<u32>,
    /// An unknown `encoding` name that was treated as 16-bit PCM.
    pub encoding_assumed_pcm: Option<String>,
    pub end_of_turn_audio_model: bool,
    pub end_of_turn_text_model: bool,
    pub silence_ceiling_ms: u32,
    pub interims: InterimMode,
    /// The deadline in force, counted from the turn's newest cut (Addendum B1).
    pub final_deadline_ms: u32,
    /// The bound within which a turn closes, counted from its last voiced sample.
    pub resolution_deadline_ms: u32,
}

/// A condition that arises during the call and does not end it.
#[derive(Debug, Clone, PartialEq)]
pub enum NoticeKind {
    DetectorSwitched {
        to: DetectorKind,
        reason: DetectorFallback,
    },
    /// The intake queue overflowed and audio was discarded.
    AudioDropped { bytes: u64 },
    /// A split segment came back with no text; the detector thresholds rose.
    NoiseThresholdRaised { to: f32 },
    /// A warning the transcriber layer returned (for example a request field the vendor refused).
    Transcriber { code: String, message: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SttNotice {
    pub kind: NoticeKind,
    pub turn_id: Option<u64>,
    pub seq: Option<u32>,
}

/// Why a unit produced no text although nothing failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilteredBy {
    /// Under the minimum speech span; never uploaded.
    TooShort,
    /// The admission hook said the speech was not input.
    NotInput,
    /// Abandoned after a split segment came back empty (noise escalation).
    NoiseSuspected,
    /// The quality check dropped the text (invented text on noise, a known hallucination).
    Quality(String),
}

impl FilteredBy {
    pub fn as_str(&self) -> &str {
        match self {
            Self::TooShort => "too_short",
            Self::NotInput => "not_input",
            Self::NoiseSuspected => "noise_suspected",
            Self::Quality(r) => r.as_str(),
        }
    }
}

/// The class of a vendor failure, from the layer that saw the HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorClass {
    /// 401 or 403: the credential was refused.
    Auth,
    /// 429, or a vendor capacity answer.
    RateLimited,
    /// The vendor said the model does not exist or is not served.
    ModelNotServed,
    /// Another 4xx: the request was refused.
    BadRequest,
    /// 5xx.
    Vendor,
    /// The connection failed or was reset.
    Network,
    /// The request or the unit reached its time limit.
    Timeout,
    /// The response could not be read.
    Protocol,
    /// The address was refused by the egress policy before anything was sent.
    EndpointRejected,
    /// The session ended while the request was running.
    Cancelled,
    /// A gateway fault.
    Internal,
}

impl ErrorClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::RateLimited => "rate_limited",
            Self::ModelNotServed => "model_not_served",
            Self::BadRequest => "bad_request",
            Self::Vendor => "vendor",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::Protocol => "protocol",
            Self::EndpointRejected => "endpoint_rejected",
            Self::Cancelled => "cancelled",
            Self::Internal => "internal",
        }
    }
}

/// Why a unit failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    Vendor(ErrorClass),
    BreakerOpen,
    LimiterRefused,
    /// An earlier unit ended the session (a refused credential); nothing was uploaded.
    SessionFatal,
    /// The session ended while the unit was in flight.
    Cancelled,
}

impl FailureClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vendor(c) => c.as_str(),
            Self::BreakerOpen => "breaker_open",
            Self::LimiterRefused => "limiter_refused",
            Self::SessionFatal => "session_fatal",
            Self::Cancelled => "cancelled",
        }
    }
}

/// How one upload unit resolved. Exactly one per unit.
#[derive(Debug, Clone, PartialEq)]
pub enum SegmentResultKind {
    Text,
    Empty,
    Filtered(FilteredBy),
    Failed(FailureClass),
    TimedOut,
}

impl SegmentResultKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Empty => "empty",
            Self::Filtered(_) => "filtered",
            Self::Failed(_) => "failed",
            Self::TimedOut => "timed_out",
        }
    }

    /// A lost unit: speech the vendor never transcribed.
    pub fn is_gap(&self) -> bool {
        matches!(self, Self::Failed(_) | Self::TimedOut)
    }
}

/// Why a segment was cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutReason {
    Pause,
    /// A run of frames that never became quiet, bounded by `max_uncertain_ms`.
    Uncertain,
    SoftSplit,
    HardSplit,
    Commit,
    InputStall,
    MaxTurnDuration,
}

impl CutReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Uncertain => "uncertain",
            Self::SoftSplit => "soft_split",
            Self::HardSplit => "hard_split",
            Self::Commit => "commit",
            Self::InputStall => "input_stall",
            Self::MaxTurnDuration => "max_turn_duration",
        }
    }

    /// A split keeps the caller's turn open; it is not a pause.
    pub fn is_split(self) -> bool {
        matches!(self, Self::SoftSplit | Self::HardSplit)
    }
}

/// Engine-clock instants (ms on the engine's monotonic clock) of one unit's life.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SegmentTimings {
    pub speech_end_ms: u64,
    pub cut_ms: u64,
    pub handed_over_ms: Option<u64>,
    pub response_ms: Option<u64>,
    pub released_ms: u64,
    /// Time the unit was held in the engine before hand-over.
    pub held_ms: u64,
    /// Time spent waiting at the limiter.
    pub queue_ms: u64,
}

/// One per upload unit, delivered at release in sequence order.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentOutcome {
    pub turn_id: u64,
    pub seq: u32,
    pub index_in_turn: u16,
    pub speech_segments: u8,
    pub turn_final: bool,
    pub voiced_ms: u32,
    /// Real audio plus padding, as uploaded once.
    pub audio_ms: u32,
    /// Where this unit's text, or its gap, sits in the turn's joined text (in chars).
    pub joined_text_offset: u32,
    /// Audio handed to the vendor over every request of the unit.
    pub uploaded_seconds: f64,
    /// What the vendor bills for those requests, by the row's billing rule.
    pub billed_seconds: f64,
    pub timings: SegmentTimings,
    pub kind: SegmentResultKind,
    /// Requests started beyond the first.
    pub retries: u8,
    pub overlapped_agent_speech: bool,
    /// The quality check marked the text as possibly invented.
    pub suspect: bool,
    pub cut: CutReason,
    /// Under one second of real audio: filtered more strictly.
    pub short: bool,
    pub detector: DetectorKind,
    pub vendor_request_id: Option<String>,
}

/// One result the engine emits. Only two shapes exist: an interim (`is_final == false`) carrying
/// the turn so far, and the turn's final with `is_final` and `is_speech_final` both true.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineResult {
    pub turn_id: u64,
    pub transcript: String,
    pub is_final: bool,
    pub is_speech_final: bool,
    /// The turn's lowest segment confidence; 1.0 when the vendor reported none.
    pub confidence: f32,
    /// What the vendor reported, and only that.
    pub vendor_confidence: Option<f32>,
    pub detected_language: Option<String>,
    /// The turn's span from its first speech sample to its last speech end, in seconds.
    pub audio_duration: Option<f64>,
    pub vendor_request_id: Option<String>,
}

/// Called on the engine task. Must return at once: implementations only enqueue.
pub type SpeechActivityCallback = Arc<dyn Fn(SpeechActivity) + Send + Sync>;
/// Asked on the engine task once per segment at its cut. Must not block.
pub type SegmentAdmissionHook = Arc<dyn Fn(&SegmentMeta) -> SegmentAdmission + Send + Sync>;
/// Called on the engine task for conditions that do not end the session.
pub type NoticeCallback = Arc<dyn Fn(SttNotice) + Send + Sync>;

/// Receives every outcome at release, in sequence order. Every method must return at once.
pub trait SegmentOutcomeSink: Send + Sync {
    fn record(&self, outcome: &SegmentOutcome);
    /// On the engine task, before the turn's final is queued.
    fn turn_closing(&self, _turn_id: u64, _closed: &SpeechActivity) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_conversions_round_to_whole_frames() {
        assert_eq!(ms_to_samples(32), 512);
        assert_eq!(samples_to_ms(16_000), 1000);
        assert_eq!(ms_to_frames_ceil(224), 7);
        assert_eq!(ms_to_frames_ceil(200), 7);
        assert_eq!(ms_to_frames_ceil(250), 8);
        assert_eq!(ms_to_frames_ceil(400), 13);
        assert_eq!(ms_to_frames_ceil(1500), 47);
    }

    #[test]
    fn only_failed_and_timed_out_units_are_gaps() {
        assert!(SegmentResultKind::TimedOut.is_gap());
        assert!(SegmentResultKind::Failed(FailureClass::LimiterRefused).is_gap());
        assert!(!SegmentResultKind::Empty.is_gap());
        assert!(!SegmentResultKind::Filtered(FilteredBy::TooShort).is_gap());
        assert!(!SegmentResultKind::Text.is_gap());
    }

    #[test]
    fn commit_and_max_turn_are_the_only_sealing_endpoints() {
        assert!(TurnCloseReason::Commit.is_sealing());
        assert!(TurnCloseReason::MaxTurnDuration.is_sealing());
        assert!(!TurnCloseReason::EndOfTurnModel.is_sealing());
        assert!(!TurnCloseReason::MaxEndpointing.is_sealing());
    }
}
