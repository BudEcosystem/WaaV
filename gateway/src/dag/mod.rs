//! DAG-based routing system for WaaV Gateway
//!
//! This module implements a Graph-based Directed Acyclic Graph (DAG) routing system
//! that enables per-connection pipeline configuration. Each WebSocket client can define
//! its own processing graph for routing audio and data through STT, TTS, LLM endpoints,
//! and custom processors.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────────┐
//! │                        DAG ROUTING ARCHITECTURE                              │
//! ├─────────────────────────────────────────────────────────────────────────────┤
//! │  DAG Definition (YAML/JSON)                                                  │
//! │         │                                                                    │
//! │         ▼                                                                    │
//! │  DAG Compiler (validation + Rhai AST compilation)                           │
//! │         │                                                                    │
//! │         ▼                                                                    │
//! │  DAG Executor (topological traversal + async spawn for parallelism)         │
//! │         │                                                                    │
//! │         ▼                                                                    │
//! │  Endpoint Adapters (HTTP, gRPC, WebSocket, IPC, LiveKit, Webhooks)          │
//! └─────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Features
//!
//! - **Per-connection configuration**: Each WebSocket client defines its own DAG
//! - **Expression-based routing**: Rhai scripting for dynamic condition evaluation
//! - **API key-based routing**: Route requests based on authenticated API key identity
//! - **Split/Join parallelism**: Execute multiple DAG branches concurrently
//! - **Multi-endpoint support**: HTTP, gRPC, WebSocket, IPC, LiveKit, Webhooks
//! - **Real-time performance**: Lock-free SPSC ring buffers, <10ms audio latency
//!
//! # Example DAG Definition
//!
//! ```yaml
//! dag:
//!   id: voice-bot-pipeline
//!   name: Simple Voice Bot
//!   version: "1.0.0"
//!
//!   nodes:
//!     - id: audio_in
//!       type: audio_input
//!     - id: stt
//!       type: stt_provider
//!       provider: deepgram
//!     - id: llm
//!       type: http_endpoint
//!       url: "https://api.openai.com/v1/chat/completions"
//!     - id: tts
//!       type: tts_provider
//!       provider: elevenlabs
//!     - id: audio_out
//!       type: audio_output
//!       destination: livekit
//!
//!   edges:
//!     - from: audio_in
//!       to: stt
//!     - from: stt
//!       to: llm
//!       condition: "is_speech_final == true"
//!     - from: llm
//!       to: tts
//!     - from: tts
//!       to: audio_out
//!
//!   entry_node: audio_in
//!   exit_nodes: [audio_out]
//! ```

#[cfg(feature = "dag-routing")]
pub mod definition;
#[cfg(feature = "dag-routing")]
pub mod compiler;
#[cfg(feature = "dag-routing")]
pub mod executor;
#[cfg(feature = "dag-routing")]
pub mod context;
#[cfg(feature = "dag-routing")]
pub mod routing;
#[cfg(feature = "dag-routing")]
pub mod metrics;
#[cfg(feature = "dag-routing")]
pub mod nodes;
#[cfg(feature = "dag-routing")]
pub mod edges;
#[cfg(feature = "dag-routing")]
pub mod endpoints;
#[cfg(feature = "dag-routing")]
pub mod error;
#[cfg(feature = "dag-routing")]
pub mod templates;

// Re-export commonly used types for convenience
#[cfg(feature = "dag-routing")]
pub use definition::{DAGDefinition, NodeDefinition, EdgeDefinition, NodeType};
#[cfg(feature = "dag-routing")]
pub use compiler::{DAGCompiler, CompiledDAG};
#[cfg(feature = "dag-routing")]
pub use executor::DAGExecutor;
#[cfg(feature = "dag-routing")]
pub use context::DAGContext;
#[cfg(feature = "dag-routing")]
pub use error::DAGError;
#[cfg(feature = "dag-routing")]
pub use templates::{global_templates, initialize_templates, DAGTemplateRegistry, TemplatesConfig, TemplateError};

/// Prelude module for convenient imports
#[cfg(feature = "dag-routing")]
pub mod prelude {
    pub use super::definition::{
        DAGDefinition, NodeDefinition, EdgeDefinition, NodeType,
        OutputDestination, HttpMethod, JoinStrategy, SwitchPattern,
        RouteDefinition, DAGConfig,
    };
    pub use super::compiler::{DAGCompiler, CompiledDAG};
    pub use super::executor::DAGExecutor;
    pub use super::context::{DAGContext, DAGTiming};
    pub use super::error::{DAGError, DAGResult};
    pub use super::nodes::prelude::*;
    pub use super::edges::prelude::*;
    pub use super::endpoints::prelude::*;
}

#[cfg(test)]
#[cfg(feature = "dag-routing")]
mod tests {
    use super::*;

    #[test]
    fn test_module_structure() {
        // Basic smoke test to ensure module compiles
        assert!(true);
    }
}
