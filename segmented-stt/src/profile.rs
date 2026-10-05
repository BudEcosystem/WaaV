//! Every engine threshold, merged from the gateway defaults, the capability row and the session.
//!
//! Values are milliseconds and are checked at 32 ms frame boundaries, so each rounds up to whole
//! frames. Defaults and their evidence are in the engine design (W1 section 3.3).

use crate::types::InterimMode;

/// When a unit is handed to the vendor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadPolicy {
    /// Upload at every pause, up to `max_in_flight` at once.
    PerPause,
    /// Hold the turn's audio and upload once, after the end-of-turn decision (rows with tight
    /// request limits, such as Azure OpenAI at default quota).
    PerTurn,
}

impl UploadPolicy {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "per_pause" | "per_segment" | "pause" => Some(Self::PerPause),
            "per_turn" | "turn" => Some(Self::PerTurn),
            _ => None,
        }
    }
}

/// How the end of a turn is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointPolicy {
    /// The audio model at the pause, the text model when the text is back, then the ceiling.
    Auto,
    /// The silence rule, then the ceiling (an agent whose turn detection is `server_vad`).
    Silence,
}

/// What a session tunes: three named fields and the policy (Addendum A4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointTuning {
    /// The pause at which audio is cut. An engine constant (224 ms); no session changes it.
    pub cut_pause_ms: u32,
    /// The least silence that ends a turn: an agent's `silence_ms`, or the canonical
    /// `endpointing_ms`. `None` keeps the default.
    pub min_end_silence_ms: Option<u32>,
    /// The silence ceiling, clamped to 800..=3000 ms. `None` keeps the default 1,500 ms.
    pub silence_ceiling_ms: Option<u32>,
    pub policy: EndpointPolicy,
}

impl Default for EndpointTuning {
    fn default() -> Self {
        Self {
            cut_pause_ms: CUT_PAUSE_MS,
            min_end_silence_ms: None,
            silence_ceiling_ms: None,
            policy: EndpointPolicy::Auto,
        }
    }
}

/// The cut pause.
pub const CUT_PAUSE_MS: u32 = 224;
/// The default silence ceiling, the streaming path's (`core/voice_manager/config.rs`).
pub const DEFAULT_SILENCE_CEILING_MS: u32 = 1500;
/// The accepted range of a session's silence ceiling.
pub const SILENCE_CEILING_RANGE: (u32, u32) = (800, 3000);

#[derive(Debug, Clone, PartialEq)]
pub struct SegmentProfile {
    pub enter_threshold: f32,
    pub exit_threshold: f32,
    pub energy_floor_rms: f32,
    pub start_confirm_ms: u32,
    pub onset_max_gap_ms: u32,
    pub onset_window_ms: u32,
    pub max_uncertain_ms: u32,
    pub pre_roll_ms: u32,
    pub cut_silence_ms: u32,
    pub trailing_silence_ms: u32,
    pub trailing_cap_ms: u32,
    pub min_voiced_ms: u32,
    pub short_segment_ms: u32,
    pub max_segment_ms: u32,
    pub split_pause_ms: u32,
    pub hard_split_overlap_ms: u32,
    pub max_in_flight: usize,
    pub upload_policy: UploadPolicy,
    pub endpoint_policy: EndpointPolicy,
    pub end_of_turn_threshold: f32,
    /// Wall clock: how long the audio verdict may take before it counts as failed.
    pub verdict_timeout_ms: u32,
    /// Wall clock: the text model's budget, the streaming path's.
    pub text_verdict_timeout_ms: u32,
    pub min_endpoint_silence_ms: u32,
    pub endpoint_silence_ms: u32,
    pub max_endpointing_ms: u32,
    pub max_turn_ms: u32,
    /// Wall clock with no audio before an open segment is cut. `None` turns the rule off.
    pub input_stall_ms: Option<u32>,
    pub noise_step: f32,
    pub noise_ceiling: f32,
    pub interims: InterimMode,
    pub ingest_budget_bytes: usize,
    /// Continuous idle audio after which the detector's recurrent state is reset.
    pub idle_reset_ms: u32,
    /// The target's shortest accepted upload; a commit of less is not uploaded.
    pub min_commit_audio_ms: u32,
    /// The longest silence put between two joined segments.
    pub join_gap_cap_ms: u32,
    /// How long the engine waits past a unit's deadline before it gives up on the call (a defect).
    pub turn_guard_ms: u32,
}

impl Default for SegmentProfile {
    fn default() -> Self {
        Self {
            enter_threshold: 0.5,
            exit_threshold: 0.35,
            energy_floor_rms: 0.008,
            start_confirm_ms: 224,
            onset_max_gap_ms: 64,
            onset_window_ms: 448,
            max_uncertain_ms: 640,
            pre_roll_ms: 400,
            cut_silence_ms: CUT_PAUSE_MS,
            trailing_silence_ms: 500,
            trailing_cap_ms: 1000,
            min_voiced_ms: 250,
            short_segment_ms: 1000,
            max_segment_ms: 25_000,
            split_pause_ms: 96,
            hard_split_overlap_ms: 320,
            max_in_flight: 2,
            upload_policy: UploadPolicy::PerPause,
            endpoint_policy: EndpointPolicy::Auto,
            end_of_turn_threshold: 0.7,
            verdict_timeout_ms: 400,
            text_verdict_timeout_ms: 100,
            min_endpoint_silence_ms: 400,
            endpoint_silence_ms: 500,
            max_endpointing_ms: DEFAULT_SILENCE_CEILING_MS,
            max_turn_ms: 60_000,
            input_stall_ms: Some(1000),
            noise_step: 0.1,
            noise_ceiling: 0.8,
            interims: InterimMode::PerSegment,
            ingest_budget_bytes: 6 * 1024 * 1024,
            idle_reset_ms: 5000,
            min_commit_audio_ms: 100,
            join_gap_cap_ms: 600,
            turn_guard_ms: 250,
        }
    }
}

impl SegmentProfile {
    /// The soft split limit: the smaller of 20 s and the maximum segment less 5 s (decision 15).
    pub fn soft_max_segment_ms(&self) -> u32 {
        20_000.min(self.max_segment_ms.saturating_sub(5000)).max(1000)
    }

    /// Apply a session's endpoint tuning (W1 section 2.7).
    pub fn with_tuning(mut self, tuning: &EndpointTuning) -> Self {
        self.cut_silence_ms = tuning.cut_pause_ms;
        let ceiling = tuning
            .silence_ceiling_ms
            .unwrap_or(self.max_endpointing_ms)
            .clamp(SILENCE_CEILING_RANGE.0, SILENCE_CEILING_RANGE.1);
        if let Some(min_end) = tuning.min_end_silence_ms {
            self.endpoint_silence_ms = min_end.clamp(self.cut_silence_ms, ceiling);
            self.min_endpoint_silence_ms = min_end.clamp(self.cut_silence_ms, ceiling);
        }
        self.max_endpointing_ms = ceiling.max(self.min_endpoint_silence_ms);
        self.endpoint_policy = tuning.policy;
        self
    }

    /// The profile tests use: every wall-clock rule off, so time is the sample clock alone.
    pub fn for_tests() -> Self {
        Self {
            input_stall_ms: None,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_soft_split_is_twenty_seconds_or_five_under_the_row_maximum() {
        let p = SegmentProfile::default();
        assert_eq!(p.soft_max_segment_ms(), 20_000);
        let amivoice = SegmentProfile { max_segment_ms: 14_000, ..p.clone() };
        assert_eq!(amivoice.soft_max_segment_ms(), 9_000);
        let fpt = SegmentProfile { max_segment_ms: 15_000, ..p };
        assert_eq!(fpt.soft_max_segment_ms(), 10_000);
    }

    #[test]
    fn a_session_ceiling_is_clamped_to_800_to_3000() {
        let low = SegmentProfile::default().with_tuning(&EndpointTuning {
            silence_ceiling_ms: Some(100),
            ..EndpointTuning::default()
        });
        assert_eq!(low.max_endpointing_ms, 800);
        let high = SegmentProfile::default().with_tuning(&EndpointTuning {
            silence_ceiling_ms: Some(9000),
            ..EndpointTuning::default()
        });
        assert_eq!(high.max_endpointing_ms, 3000);
    }

    #[test]
    fn endpointing_ms_sets_the_least_end_silence_but_never_the_cut_pause() {
        let p = SegmentProfile::default().with_tuning(&EndpointTuning {
            min_end_silence_ms: Some(10),
            ..EndpointTuning::default()
        });
        assert_eq!(p.cut_silence_ms, 224, "a small endpointing_ms cannot multiply uploads");
        assert_eq!(p.min_endpoint_silence_ms, 224);
        assert_eq!(p.endpoint_silence_ms, 224);
        let p = SegmentProfile::default().with_tuning(&EndpointTuning {
            min_end_silence_ms: Some(700),
            ..EndpointTuning::default()
        });
        assert_eq!(p.endpoint_silence_ms, 700);
        assert_eq!(p.max_endpointing_ms, 1500);
    }

    #[test]
    fn upload_policy_names() {
        assert_eq!(UploadPolicy::parse("per_turn"), Some(UploadPolicy::PerTurn));
        assert_eq!(UploadPolicy::parse("PER_PAUSE"), Some(UploadPolicy::PerPause));
        assert_eq!(UploadPolicy::parse("x"), None);
    }
}
