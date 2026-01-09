//! Endpoint adapters for external integrations
//!
//! This module provides adapter implementations for various endpoint types
//! including HTTP and Webhook.
//!
//! Note: gRPC, WebSocket, IPC, and LiveKit endpoints are implemented as
//! DAGNode types in the `nodes/endpoint.rs` module, not as EndpointAdapter
//! implementations. This is the preferred integration pattern.

mod http;
mod webhook;

pub use http::HttpAdapter;
pub use webhook::WebhookAdapter;

use std::time::Duration;
use async_trait::async_trait;

use crate::dag::context::DAGContext;
use crate::dag::nodes::DAGData;
use crate::dag::error::DAGResult;

/// Prelude for convenient imports
pub mod prelude {
    pub use super::{
        EndpointAdapter,
        HttpAdapter,
        WebhookAdapter,
    };
}

/// Trait for endpoint adapters
///
/// All endpoint types must implement this trait to be usable
/// as DAG endpoints.
#[async_trait]
pub trait EndpointAdapter: Send + Sync {
    /// Get the endpoint type name
    fn endpoint_type(&self) -> &str;

    /// Get the endpoint identifier
    fn endpoint_id(&self) -> &str;

    /// Send data to the endpoint and receive response
    async fn send(&self, data: DAGData, ctx: &DAGContext) -> DAGResult<DAGData>;

    /// Check if the endpoint is connected
    fn is_connected(&self) -> bool;

    /// Connect to the endpoint
    async fn connect(&mut self) -> DAGResult<()>;

    /// Disconnect from the endpoint
    async fn disconnect(&mut self) -> DAGResult<()>;

    /// Get the configured timeout
    fn timeout(&self) -> Duration;
}
