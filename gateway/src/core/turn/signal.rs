//! The normalized, provider-agnostic turn signal.
//!
//! Built ONCE at the VoiceManager seam (STT result wrapper, smart-turn
//! fire-site, TTS egress) — never per-provider. All ~33 STT providers feed
//! this stream through `BaseSTT::on_result`; the smart-turn ensemble feeds it
//! through its turn-complete verdict; the TTS egress feeds the bot-speaking
//! edges.

/// One normalized signal into the [`super::TurnController`].
#[derive(Debug, Clone, PartialEq)]
pub enum ControllerSignal {
    /// An interim (mutable) transcript fragment.
    SttInterim { text: String, confidence: f32 },
    /// A final (immutable) transcript. `is_speech_final` = the provider's (or
    /// the timer/turn-detector's forced) end-of-utterance; `is_finalized` =
    /// the provider explicitly acked a finalize handshake (A-G1) and has
    /// nothing more for this segment.
    SttFinal {
        text: String,
        is_speech_final: bool,
        is_finalized: bool,
    },
    /// The smart-turn ensemble's verdict, consumed OPAQUE
    /// (`TurnDecisionEngine.is_turn_complete`) — see the module docs for why
    /// no raw probability crosses this boundary.
    SmartTurn { is_complete: bool },
    /// First TTS audio chunk crossed the egress boundary (the bot is audibly
    /// starting). NOT `on_complete` — that fires at *generation* end while
    /// audio is still playing out.
    BotStarted,
    /// Bot playback drained (or the closest available approximation at the
    /// wiring site — see PIPECAT_FIX_PLAN §6.2 A7).
    BotStopped,
    /// The STT provider's measured speech-end→final p99 (D-G8), for
    /// TTFS-aware stop strategies (A-G2).
    SttMetadata { ttfs_p99_ms: u64 },
    /// A controller-owned timer expired (stop strategies arm timers through
    /// the controller; the expiry is fed back as a signal under the same
    /// serialized path as everything else — no strategy owns a task).
    Timer { generation: u64 },
}

impl ControllerSignal {
    /// Signals representing USER INPUT — these (and only these) are dropped
    /// while a mute strategy is active. Bot/lifecycle/metadata signals are
    /// never muted (Pipecat's lifecycle exemption).
    pub fn is_user_input(&self) -> bool {
        matches!(
            self,
            ControllerSignal::SttInterim { .. }
                | ControllerSignal::SttFinal { .. }
                | ControllerSignal::SmartTurn { .. }
        )
    }
}
