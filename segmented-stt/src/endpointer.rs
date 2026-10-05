//! The end-of-turn ladder.
//!
//! Three rungs, each cheaper to reach than the next: the audio end-of-turn model asked once at the
//! pause; if it says "not finished" (or failed), the text end-of-turn model once the transcript is
//! back; otherwise the silence ceiling, 1,500 ms by default, the streaming path's. With no audio
//! model, or for an agent whose turn detection is silence-based, the silence rule ends the turn
//! after `endpoint_silence_ms`.

use crate::profile::{EndpointPolicy, SegmentProfile};
use crate::types::{FRAME_MS, TurnCloseReason, ms_to_frames_ceil};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("end-of-turn model failed: {0}")]
pub struct EndOfTurnError(pub String);

/// The audio end-of-turn model (SmartTurn), asked on demand once per pause.
#[async_trait::async_trait]
pub trait EndOfTurnModel: Send + Sync {
    /// Probability that the caller has finished, from at most 8 s of 16 kHz audio ending now.
    async fn completion_probability(&self, audio_16k: Vec<f32>) -> Result<f32, EndOfTurnError>;
}

/// The text end-of-turn model the gateway already loads.
#[async_trait::async_trait]
pub trait EndOfTurnTextModel: Send + Sync {
    async fn is_complete(&self, turn_text: &str) -> Result<bool, EndOfTurnError>;
}

/// The audio verdict of one pause.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    Pending,
    Probability(f32),
    /// Failed or timed out.
    Failed,
}

/// What the ladder reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LadderInput {
    pub policy: EndpointPolicy,
    pub audio_model: bool,
    /// The verdict of the deciding pause (the latest pause that follows a unit with text; while
    /// the latest pause's unit is out, that pause's own verdict).
    pub verdict: Option<Verdict>,
    pub all_released: bool,
    /// The text model's answer, once asked.
    pub text_complete: Option<bool>,
    /// Silence on the sample clock since the last voiced frame, plus input-idle wall time.
    pub effective_silence_ms: u32,
    /// The silence that reached the ceiling was input-idle time.
    pub input_idle: bool,
}

/// Thresholds rounded up to whole frames, as the segmenter checks them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LadderThresholds {
    pub end_of_turn_threshold: f32,
    pub endpoint_silence_ms: u32,
    pub min_endpoint_silence_ms: u32,
    pub max_endpointing_ms: u32,
}

impl LadderThresholds {
    pub fn from_profile(p: &SegmentProfile) -> Self {
        let f = |ms: u32| ms_to_frames_ceil(ms) * FRAME_MS;
        Self {
            end_of_turn_threshold: p.end_of_turn_threshold,
            endpoint_silence_ms: f(p.endpoint_silence_ms),
            min_endpoint_silence_ms: f(p.min_endpoint_silence_ms),
            max_endpointing_ms: f(p.max_endpointing_ms),
        }
    }
}

/// Whether the text rung should be asked now.
pub fn wants_text_verdict(i: &LadderInput, t: &LadderThresholds, text_model: bool) -> bool {
    if !text_model
        || i.policy != EndpointPolicy::Auto
        || !i.all_released
        || i.text_complete.is_some()
    {
        return false;
    }
    match i.verdict {
        Some(Verdict::Probability(p)) => p < t.end_of_turn_threshold,
        Some(Verdict::Failed) => true,
        Some(Verdict::Pending) => false,
        None => !i.audio_model,
    }
}

/// The endpoint in force, if any.
pub fn decide(i: &LadderInput, t: &LadderThresholds) -> Option<TurnCloseReason> {
    if i.policy == EndpointPolicy::Auto
        && i.audio_model
        && let Some(Verdict::Probability(p)) = i.verdict
        && p >= t.end_of_turn_threshold
    {
        return Some(TurnCloseReason::EndOfTurnModel);
    }
    if i.policy == EndpointPolicy::Auto && i.all_released && i.text_complete == Some(true) {
        return Some(TurnCloseReason::TextModel);
    }
    if (!i.audio_model || i.policy == EndpointPolicy::Silence)
        && i.effective_silence_ms >= t.endpoint_silence_ms
    {
        return Some(TurnCloseReason::SilenceThreshold);
    }
    if i.effective_silence_ms >= t.max_endpointing_ms {
        return Some(if i.input_idle {
            TurnCloseReason::InputIdle
        } else {
            TurnCloseReason::MaxEndpointing
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> LadderThresholds {
        LadderThresholds::from_profile(&SegmentProfile::default())
    }

    fn input() -> LadderInput {
        LadderInput {
            policy: EndpointPolicy::Auto,
            audio_model: true,
            verdict: Some(Verdict::Pending),
            all_released: false,
            text_complete: None,
            effective_silence_ms: 224,
            input_idle: false,
        }
    }

    #[test]
    fn thresholds_round_up_to_whole_frames() {
        let t = t();
        assert_eq!(t.endpoint_silence_ms, 512);
        assert_eq!(t.min_endpoint_silence_ms, 416);
        assert_eq!(t.max_endpointing_ms, 1504);
    }

    #[test]
    fn a_finished_audio_verdict_decides_at_once() {
        let i = LadderInput {
            verdict: Some(Verdict::Probability(0.8)),
            ..input()
        };
        assert_eq!(decide(&i, &t()), Some(TurnCloseReason::EndOfTurnModel));
        let i = LadderInput {
            verdict: Some(Verdict::Probability(0.69)),
            ..input()
        };
        assert_eq!(decide(&i, &t()), None);
    }

    #[test]
    fn the_text_model_is_asked_only_after_every_unit_is_released_and_the_audio_said_no() {
        let i = LadderInput {
            verdict: Some(Verdict::Probability(0.66)),
            all_released: true,
            ..input()
        };
        assert!(wants_text_verdict(&i, &t(), true));
        assert!(!wants_text_verdict(&i, &t(), false));
        assert!(!wants_text_verdict(
            &LadderInput {
                all_released: false,
                ..i
            },
            &t(),
            true
        ));
        let done = LadderInput {
            text_complete: Some(true),
            ..i
        };
        assert_eq!(decide(&done, &t()), Some(TurnCloseReason::TextModel));
        let failed = LadderInput {
            verdict: Some(Verdict::Failed),
            all_released: true,
            ..input()
        };
        assert!(wants_text_verdict(&failed, &t(), true));
    }

    #[test]
    fn without_an_audio_model_the_silence_rule_applies() {
        let i = LadderInput {
            audio_model: false,
            verdict: None,
            effective_silence_ms: 480,
            ..input()
        };
        assert_eq!(decide(&i, &t()), None);
        let i = LadderInput {
            effective_silence_ms: 512,
            ..i
        };
        assert_eq!(decide(&i, &t()), Some(TurnCloseReason::SilenceThreshold));
    }

    #[test]
    fn a_silence_policy_ignores_the_audio_model() {
        let i = LadderInput {
            policy: EndpointPolicy::Silence,
            verdict: Some(Verdict::Probability(0.95)),
            effective_silence_ms: 300,
            ..input()
        };
        assert_eq!(decide(&i, &t()), None);
        let i = LadderInput {
            effective_silence_ms: 512,
            ..i
        };
        assert_eq!(decide(&i, &t()), Some(TurnCloseReason::SilenceThreshold));
    }

    #[test]
    fn the_ceiling_ends_every_turn() {
        let i = LadderInput {
            verdict: Some(Verdict::Probability(0.1)),
            effective_silence_ms: 1504,
            ..input()
        };
        assert_eq!(decide(&i, &t()), Some(TurnCloseReason::MaxEndpointing));
        let idle = LadderInput {
            input_idle: true,
            ..i
        };
        assert_eq!(decide(&idle, &t()), Some(TurnCloseReason::InputIdle));
    }
}
