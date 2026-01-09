//! DAG error types
//!
//! Centralized error handling for the DAG routing system with detailed error variants
//! for compilation, execution, and routing failures.

use std::fmt;
use thiserror::Error;

/// Result type for DAG operations
pub type DAGResult<T> = Result<T, DAGError>;

/// Comprehensive error type for DAG routing system
#[derive(Error, Debug)]
pub enum DAGError {
    // ─────────────────────────────────────────────────────────────────────────────
    // Compilation Errors
    // ─────────────────────────────────────────────────────────────────────────────

    /// DAG definition parsing failed
    #[error("Failed to parse DAG definition: {0}")]
    ParseError(String),

    /// Unknown node referenced in edge or configuration
    #[error("Unknown node ID: {0}")]
    UnknownNode(String),

    /// Duplicate node ID in DAG definition
    #[error("Duplicate node ID: {0}")]
    DuplicateNodeId(String),

    /// Cycle detected in DAG (violates acyclicity constraint)
    #[error("Cycle detected in DAG at node: {0}")]
    CycleDetected(String),

    /// Entry node not found in DAG
    #[error("Entry node '{0}' not found in DAG")]
    EntryNodeNotFound(String),

    /// Exit node not found in DAG
    #[error("Exit node '{0}' not found in DAG")]
    ExitNodeNotFound(String),

    /// Invalid DAG structure (disconnected components, etc.)
    #[error("Invalid DAG structure: {0}")]
    InvalidStructure(String),

    /// Rhai expression compilation failed
    #[error("Failed to compile expression '{expression}': {error}")]
    ExpressionCompilationError {
        expression: String,
        error: String,
    },

    /// Invalid switch pattern configuration
    #[error("Invalid switch pattern: {0}")]
    InvalidSwitchPattern(String),

    // ─────────────────────────────────────────────────────────────────────────────
    // Configuration Errors
    // ─────────────────────────────────────────────────────────────────────────────

    /// Required configuration field missing
    #[error("Missing required configuration: {0}")]
    MissingConfiguration(String),

    /// Invalid node configuration
    #[error("Invalid node configuration for '{node_id}': {error}")]
    InvalidNodeConfig {
        node_id: String,
        error: String,
    },

    /// Configuration validation error (used for security checks, input validation)
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Provider not found or not registered
    #[error("Provider '{0}' not found in registry")]
    ProviderNotFound(String),

    /// Unsupported node type
    #[error("Unsupported node type: {0}")]
    UnsupportedNodeType(String),

    // ─────────────────────────────────────────────────────────────────────────────
    // Execution Errors
    // ─────────────────────────────────────────────────────────────────────────────

    /// Node execution failed
    #[error("Node '{node_id}' execution failed: {error}")]
    NodeExecutionError {
        node_id: String,
        error: String,
    },

    /// Condition evaluation failed
    #[error("Condition evaluation failed: {0}")]
    ConditionError(String),

    /// Edge transform execution failed
    #[error("Edge transform on '{edge}' failed: {error}")]
    TransformError {
        edge: String,
        error: String,
    },

    /// No matching route found for current state
    #[error("No matching route found from node '{0}'")]
    NoMatchingRoute(String),

    /// Invalid start node for execution
    #[error("Invalid start node for execution")]
    InvalidStartNode,

    /// Empty join (no results to aggregate)
    #[error("Join node received no results to aggregate")]
    EmptyJoin,

    /// Join operation timed out
    #[error("Join operation timed out after {0} ms")]
    JoinTimeout(u64),

    /// Split branch execution failed
    #[error("Split branch '{branch_id}' failed: {error}")]
    SplitBranchError {
        branch_id: String,
        error: String,
    },

    /// DAG execution cancelled
    #[error("DAG execution was cancelled")]
    Cancelled,

    /// Execution timeout exceeded
    #[error("Execution timeout exceeded ({0} ms)")]
    ExecutionTimeout(u64),

    // ─────────────────────────────────────────────────────────────────────────────
    // Data Errors
    // ─────────────────────────────────────────────────────────────────────────────

    /// Unsupported data type for operation
    #[error("Unsupported data type for operation: expected {expected}, got {actual}")]
    UnsupportedDataType {
        expected: String,
        actual: String,
    },

    /// Data serialization failed
    #[error("Data serialization failed: {0}")]
    SerializationError(String),

    /// Data deserialization failed
    #[error("Data deserialization failed: {0}")]
    DeserializationError(String),

    /// Field extraction failed (for switch patterns)
    #[error("Failed to extract field '{field}' from data: {error}")]
    FieldExtractionError {
        field: String,
        error: String,
    },

    // ─────────────────────────────────────────────────────────────────────────────
    // Endpoint Errors
    // ─────────────────────────────────────────────────────────────────────────────

    /// HTTP endpoint request failed
    #[error("HTTP endpoint '{url}' request failed: {error}")]
    HttpEndpointError {
        url: String,
        error: String,
    },

    /// gRPC endpoint request failed
    #[error("gRPC endpoint '{service}/{method}' request failed: {error}")]
    GrpcEndpointError {
        service: String,
        method: String,
        error: String,
    },

    /// WebSocket endpoint connection failed
    #[error("WebSocket endpoint '{url}' connection failed: {error}")]
    WebSocketEndpointError {
        url: String,
        error: String,
    },

    /// IPC endpoint communication failed
    #[error("IPC endpoint '{name}' communication failed: {error}")]
    IpcEndpointError {
        name: String,
        error: String,
    },

    /// LiveKit endpoint operation failed
    #[error("LiveKit endpoint operation failed: {0}")]
    LiveKitEndpointError(String),

    /// Webhook delivery failed
    #[error("Webhook delivery to '{url}' failed: {error}")]
    WebhookDeliveryError {
        url: String,
        error: String,
    },

    /// Endpoint connection timeout
    #[error("Endpoint connection timeout: {0}")]
    EndpointTimeout(String),

    // ─────────────────────────────────────────────────────────────────────────────
    // Buffer Errors
    // ─────────────────────────────────────────────────────────────────────────────

    /// Ring buffer full (backpressure)
    #[error("Ring buffer full for edge {from} -> {to}")]
    BufferFull {
        from: String,
        to: String,
    },

    /// Ring buffer allocation failed
    #[error("Failed to allocate ring buffer of size {size}: {error}")]
    BufferAllocationError {
        size: usize,
        error: String,
    },

    // ─────────────────────────────────────────────────────────────────────────────
    // Provider Errors
    // ─────────────────────────────────────────────────────────────────────────────

    /// STT provider error
    #[error("STT provider '{provider}' error: {error}")]
    STTProviderError {
        provider: String,
        error: String,
    },

    /// TTS provider error
    #[error("TTS provider '{provider}' error: {error}")]
    TTSProviderError {
        provider: String,
        error: String,
    },

    /// Realtime provider error
    #[error("Realtime provider '{provider}' error: {error}")]
    RealtimeProviderError {
        provider: String,
        error: String,
    },

    /// Audio processor error
    #[error("Audio processor '{processor}' error: {error}")]
    AudioProcessorError {
        processor: String,
        error: String,
    },

    // ─────────────────────────────────────────────────────────────────────────────
    // Internal Errors
    // ─────────────────────────────────────────────────────────────────────────────

    /// Internal error (should not occur in normal operation)
    #[error("Internal DAG error: {0}")]
    InternalError(String),

    /// Plugin panic caught during execution
    #[error("Plugin panicked during execution: {0}")]
    PluginPanic(String),
}

impl DAGError {
    /// Create a node execution error
    pub fn node_error(node_id: impl Into<String>, error: impl fmt::Display) -> Self {
        Self::NodeExecutionError {
            node_id: node_id.into(),
            error: error.to_string(),
        }
    }

    /// Create an expression compilation error
    pub fn expression_error(expression: impl Into<String>, error: impl fmt::Display) -> Self {
        Self::ExpressionCompilationError {
            expression: expression.into(),
            error: error.to_string(),
        }
    }

    /// Create an HTTP endpoint error
    pub fn http_error(url: impl Into<String>, error: impl fmt::Display) -> Self {
        Self::HttpEndpointError {
            url: url.into(),
            error: error.to_string(),
        }
    }

    /// Create a data type error
    pub fn data_type_error(expected: impl Into<String>, actual: impl Into<String>) -> Self {
        Self::UnsupportedDataType {
            expected: expected.into(),
            actual: actual.into(),
        }
    }

    /// Create an audio processor error
    pub fn processor_error(processor: impl Into<String>, error: impl std::fmt::Display) -> Self {
        Self::AudioProcessorError {
            processor: processor.into(),
            error: error.to_string(),
        }
    }

    /// Check if this error is recoverable (can be retried)
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::BufferFull { .. }
                | Self::EndpointTimeout(_)
                | Self::HttpEndpointError { .. }
                | Self::GrpcEndpointError { .. }
                | Self::WebSocketEndpointError { .. }
        )
    }

    /// Check if this error is a configuration error (compile-time)
    pub fn is_config_error(&self) -> bool {
        matches!(
            self,
            Self::ParseError(_)
                | Self::UnknownNode(_)
                | Self::DuplicateNodeId(_)
                | Self::CycleDetected(_)
                | Self::EntryNodeNotFound(_)
                | Self::ExitNodeNotFound(_)
                | Self::InvalidStructure(_)
                | Self::ExpressionCompilationError { .. }
                | Self::InvalidSwitchPattern(_)
                | Self::MissingConfiguration(_)
                | Self::InvalidNodeConfig { .. }
        )
    }

    /// Check if this error is a runtime error (execution-time)
    pub fn is_runtime_error(&self) -> bool {
        !self.is_config_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_creation() {
        let err = DAGError::node_error("stt_node", "connection failed");
        assert!(matches!(err, DAGError::NodeExecutionError { .. }));
        assert!(err.is_runtime_error());
        assert!(!err.is_config_error());
    }

    #[test]
    fn test_config_error_classification() {
        let err = DAGError::CycleDetected("node_a".to_string());
        assert!(err.is_config_error());
        assert!(!err.is_runtime_error());
    }

    #[test]
    fn test_recoverable_error_classification() {
        let err = DAGError::EndpointTimeout("http://example.com".to_string());
        assert!(err.is_recoverable());

        let err = DAGError::CycleDetected("node_a".to_string());
        assert!(!err.is_recoverable());
    }

    #[test]
    fn test_error_display() {
        let err = DAGError::UnknownNode("missing_node".to_string());
        assert_eq!(err.to_string(), "Unknown node ID: missing_node");
    }
}
