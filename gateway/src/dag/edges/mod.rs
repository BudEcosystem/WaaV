//! DAG edge types and ring buffer management
//!
//! This module handles the connections between nodes including:
//! - Conditional edge routing
//! - Lock-free ring buffers for real-time data flow
//! - Switch pattern matching
//! - Wait-free SPSC audio buffers (rtrb)

mod condition;
mod switch;
mod buffer;

pub use condition::{EdgeCondition, CompiledEdge};
pub use switch::SwitchMatcher;
pub use buffer::{EdgeBuffer, EdgeBufferPair, RtrbAudioBuffer, RtrbAudioProducer, RtrbAudioConsumer};

/// Prelude for convenient imports
pub mod prelude {
    pub use super::condition::{EdgeCondition, CompiledEdge};
    pub use super::switch::SwitchMatcher;
    pub use super::buffer::{EdgeBuffer, EdgeBufferPair, RtrbAudioBuffer, RtrbAudioProducer, RtrbAudioConsumer};
}
