//! Types a gateway-endpointed speech-to-text provider reports: detector-timed speech events, the
//! admission hook, flush outcomes, notices, upload outcomes and facts.
//!
//! They live in the `waav-segmented-stt` crate with the engine that produces them; this module
//! re-exports them so `base.rs` does not depend on the engine.

pub use waav_segmented_stt::types::{
    CutReason, DetectorFallback, DetectorKind, ErrorClass, FailureClass, FilteredBy, FlushOutcome,
    InterimMode, NoticeCallback, NoticeKind, SegmentAdmission, SegmentAdmissionHook, SegmentMeta,
    SegmentOutcome, SegmentOutcomeSink, SegmentResultKind, SpeechActivity, SpeechActivityCallback,
    SttLiveFacts, SttNotice, TurnCloseReason,
};
