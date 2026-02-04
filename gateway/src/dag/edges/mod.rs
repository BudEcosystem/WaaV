//! DAG edge types and ring buffer management
//!
//! This module handles the connections between nodes including:
//! - Conditional edge routing
//! - Lock-free ring buffers for real-time data flow
//! - Switch pattern matching
//! - Wait-free SPSC audio buffers (rtrb)

mod buffer;
mod condition;
mod switch;

pub use buffer::{
    EdgeBuffer, EdgeBufferPair, RtrbAudioBuffer, RtrbAudioConsumer, RtrbAudioProducer,
};
pub use condition::{CompiledEdge, EdgeCondition};
pub use switch::SwitchMatcher;

/// Prelude for convenient imports
pub mod prelude {
    pub use super::buffer::{
        EdgeBuffer, EdgeBufferPair, RtrbAudioBuffer, RtrbAudioConsumer, RtrbAudioProducer,
    };
    pub use super::condition::{CompiledEdge, EdgeCondition};
    pub use super::switch::SwitchMatcher;
}
