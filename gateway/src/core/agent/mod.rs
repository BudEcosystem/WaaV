//! Voice agents (spec 025): a Bud agent — budprompt's prompt, tools and governance — spoken to
//! through an STT and a TTS deployment.
//!
//! WaaV runs the cascade (speech in, turn-taking, speech out); the agent runs in budprompt, one run
//! per user turn, reached through budgateway with the caller's own credential. Nothing here knows the
//! agent's prompt or tools: the `voice_agent` projection budapp publishes names the two deployments
//! and how the call should feel, and that is all WaaV holds.

pub mod brain;
pub mod engine;
pub mod spoken;
pub mod text;
pub mod tone;

pub use brain::{AgentBrain, AgentEvent, AgentTurnRequest, Truncate, TurnFailure};
pub use engine::{
    AgentEngine, AgentSessionConfig, AgentSignal, SpeechOut, TurnBackend, TurnKind, TurnStatus,
    is_ignored_phrase,
};
