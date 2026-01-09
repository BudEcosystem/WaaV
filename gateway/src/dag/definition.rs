//! DAG definition types for YAML/JSON configuration
//!
//! This module defines the schema for DAG configurations that users provide
//! to customize their voice processing pipelines. The definitions are designed
//! to be serializable/deserializable with serde for YAML and JSON support.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Default ring buffer capacity (samples)
const fn default_buffer_capacity() -> usize {
    4096
}

/// Default timeout for node execution (milliseconds)
const fn default_node_timeout_ms() -> u64 {
    30000
}

/// Default maximum concurrent executions
const fn default_max_concurrent() -> usize {
    10
}

/// Complete DAG definition for a processing pipeline
///
/// This is the top-level structure that users provide via WebSocket config
/// or YAML configuration files. It defines the complete graph structure
/// including nodes, edges, routing rules, and configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DAGDefinition {
    /// Unique identifier for this DAG
    pub id: String,

    /// Human-readable name
    pub name: String,

    /// Version for compatibility checking (semver format)
    #[serde(default = "default_version")]
    pub version: String,

    /// Node definitions
    pub nodes: Vec<NodeDefinition>,

    /// Edge definitions (connections between nodes)
    pub edges: Vec<EdgeDefinition>,

    /// Default input node ID
    pub entry_node: String,

    /// Default output node IDs
    pub exit_nodes: Vec<String>,

    /// API key routing rules (optional)
    /// Maps API key ID patterns to node IDs for custom routing
    #[serde(default)]
    pub api_key_routes: HashMap<String, String>,

    /// Global configuration for all nodes
    #[serde(default)]
    pub config: DAGConfig,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

impl DAGDefinition {
    /// Create a new DAG definition with minimal configuration
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            version: default_version(),
            nodes: Vec::new(),
            edges: Vec::new(),
            entry_node: String::new(),
            exit_nodes: Vec::new(),
            api_key_routes: HashMap::new(),
            config: DAGConfig::default(),
        }
    }

    /// Add a node to the DAG
    pub fn add_node(&mut self, node: NodeDefinition) -> &mut Self {
        self.nodes.push(node);
        self
    }

    /// Add an edge to the DAG
    pub fn add_edge(&mut self, edge: EdgeDefinition) -> &mut Self {
        self.edges.push(edge);
        self
    }

    /// Set the entry node
    pub fn with_entry(&mut self, node_id: impl Into<String>) -> &mut Self {
        self.entry_node = node_id.into();
        self
    }

    /// Add an exit node
    pub fn add_exit(&mut self, node_id: impl Into<String>) -> &mut Self {
        self.exit_nodes.push(node_id.into());
        self
    }

    /// Get a node by ID
    pub fn get_node(&self, id: &str) -> Option<&NodeDefinition> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Get all edges from a specific node
    pub fn get_edges_from(&self, node_id: &str) -> Vec<&EdgeDefinition> {
        self.edges.iter().filter(|e| e.from == node_id).collect()
    }

    /// Get all edges to a specific node
    pub fn get_edges_to(&self, node_id: &str) -> Vec<&EdgeDefinition> {
        self.edges.iter().filter(|e| e.to == node_id).collect()
    }

    /// Validate the DAG definition structure (basic validation)
    ///
    /// Checks for:
    /// - Non-empty nodes list
    /// - Non-empty entry_node and exit_nodes
    /// - Entry/exit node references exist
    /// - All edge references exist
    /// - No duplicate node IDs
    /// - Valid node count (max 1000 nodes)
    /// - Valid edge count (max 5000 edges)
    pub fn validate_structure(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        // === Empty DAG validation ===

        // Check for empty nodes list
        if self.nodes.is_empty() {
            errors.push("DAG has no nodes defined".to_string());
        }

        // Check for empty entry_node
        if self.entry_node.is_empty() {
            errors.push("DAG entry_node is empty".to_string());
        }

        // Check for empty exit_nodes
        if self.exit_nodes.is_empty() {
            errors.push("DAG has no exit_nodes defined".to_string());
        }

        // === Size limits (prevent DoS) ===

        const MAX_NODES: usize = 1000;
        const MAX_EDGES: usize = 5000;

        if self.nodes.len() > MAX_NODES {
            errors.push(format!(
                "DAG has too many nodes: {} (max {})",
                self.nodes.len(),
                MAX_NODES
            ));
        }

        if self.edges.len() > MAX_EDGES {
            errors.push(format!(
                "DAG has too many edges: {} (max {})",
                self.edges.len(),
                MAX_EDGES
            ));
        }

        // === Node ID validation ===

        // Check for empty node IDs
        for (i, node) in self.nodes.iter().enumerate() {
            if node.id.is_empty() {
                errors.push(format!("Node at index {} has empty ID", i));
            }
        }

        // Check for empty exit node IDs
        for (i, exit) in self.exit_nodes.iter().enumerate() {
            if exit.is_empty() {
                errors.push(format!("Exit node at index {} has empty ID", i));
            }
        }

        // === Reference validation (only if we have nodes) ===

        if !self.nodes.is_empty() {
            // Check entry node exists
            if !self.entry_node.is_empty()
                && !self.nodes.iter().any(|n| n.id == self.entry_node)
            {
                errors.push(format!(
                    "Entry node '{}' not found in nodes",
                    self.entry_node
                ));
            }

            // Check all exit nodes exist
            for exit in &self.exit_nodes {
                if !exit.is_empty() && !self.nodes.iter().any(|n| n.id == *exit) {
                    errors.push(format!("Exit node '{}' not found in nodes", exit));
                }
            }

            // Check all edge references exist
            for edge in &self.edges {
                if !self.nodes.iter().any(|n| n.id == edge.from) {
                    errors.push(format!("Edge source '{}' not found in nodes", edge.from));
                }
                if !self.nodes.iter().any(|n| n.id == edge.to) {
                    errors.push(format!("Edge target '{}' not found in nodes", edge.to));
                }
            }
        }

        // === Duplicate detection ===

        // Check for duplicate node IDs
        let mut seen_ids = std::collections::HashSet::new();
        for node in &self.nodes {
            if !node.id.is_empty() && !seen_ids.insert(&node.id) {
                errors.push(format!("Duplicate node ID: '{}'", node.id));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Global DAG configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DAGConfig {
    /// Default timeout for node execution (milliseconds)
    #[serde(default = "default_node_timeout_ms")]
    pub node_timeout_ms: u64,

    /// Maximum concurrent executions per connection
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_executions: usize,

    /// Enable execution metrics collection
    #[serde(default)]
    pub enable_metrics: bool,

    /// Enable detailed execution tracing
    #[serde(default)]
    pub enable_tracing: bool,

    /// Default ring buffer capacity for edges
    #[serde(default = "default_buffer_capacity")]
    pub default_buffer_capacity: usize,

    /// Global variable definitions (accessible in all expressions)
    #[serde(default)]
    pub variables: HashMap<String, serde_json::Value>,
}

/// Definition for a single node in the DAG
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeDefinition {
    /// Unique node identifier within this DAG
    pub id: String,

    /// Node type (determines processing behavior)
    #[serde(flatten)]
    pub node_type: NodeType,

    /// Node-specific configuration
    #[serde(default)]
    pub config: serde_json::Value,

    /// Execution timeout override (milliseconds)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,

    /// Enable retries on failure
    #[serde(default)]
    pub retry_on_failure: bool,

    /// Maximum retry attempts
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

fn default_max_retries() -> u32 {
    3
}

impl NodeDefinition {
    /// Create a new node definition
    pub fn new(id: impl Into<String>, node_type: NodeType) -> Self {
        Self {
            id: id.into(),
            node_type,
            config: serde_json::Value::Null,
            timeout_ms: None,
            retry_on_failure: false,
            max_retries: default_max_retries(),
        }
    }

    /// Set node-specific configuration
    pub fn with_config(mut self, config: serde_json::Value) -> Self {
        self.config = config;
        self
    }

    /// Enable retries with specified max attempts
    pub fn with_retries(mut self, max_retries: u32) -> Self {
        self.retry_on_failure = true;
        self.max_retries = max_retries;
        self
    }
}

/// Node type enum with associated configuration
///
/// Each variant represents a different type of processing node in the DAG.
/// The `#[serde(tag = "type")]` attribute enables YAML/JSON like:
/// ```yaml
/// - id: my_stt
///   type: stt_provider
///   provider: deepgram
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeType {
    /// Audio input from WebSocket/LiveKit
    AudioInput,

    /// Text input from WebSocket messages
    TextInput,

    /// Audio output to WebSocket/LiveKit
    AudioOutput {
        #[serde(default)]
        destination: OutputDestination,
    },

    /// Text output (STT results, responses)
    TextOutput {
        #[serde(default)]
        destination: OutputDestination,
    },

    /// STT provider node
    SttProvider {
        provider: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        language: Option<String>,
    },

    /// TTS provider node
    TtsProvider {
        provider: String,
        #[serde(default)]
        voice_id: Option<String>,
        #[serde(default)]
        model: Option<String>,
    },

    /// Realtime provider node (e.g., OpenAI Realtime)
    RealtimeProvider {
        provider: String,
        #[serde(default)]
        model: Option<String>,
    },

    /// Plugin-based audio/text processor
    Processor {
        plugin: String,
    },

    /// HTTP endpoint call
    HttpEndpoint {
        url: String,
        #[serde(default)]
        method: HttpMethod,
        #[serde(default)]
        headers: HashMap<String, String>,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },

    /// gRPC endpoint call
    GrpcEndpoint {
        address: String,
        service: String,
        method: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },

    /// WebSocket client endpoint
    WebSocketEndpoint {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },

    /// IPC shared memory endpoint
    IpcEndpoint {
        shm_name: String,
        #[serde(default)]
        input_format: Option<String>,
        #[serde(default)]
        output_format: Option<String>,
    },

    /// LiveKit WebRTC endpoint
    LiveKitEndpoint {
        #[serde(default)]
        room: Option<String>,
        #[serde(default)]
        track_type: Option<String>,
    },

    /// Webhook notification (fire-and-forget)
    WebhookOutput {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },

    /// Split node (broadcast to multiple branches)
    Split {
        branches: Vec<String>,
    },

    /// Join node (aggregate from multiple branches)
    Join {
        sources: Vec<String>,
        #[serde(default)]
        strategy: JoinStrategy,
        /// Rhai selector expression for Best strategy
        #[serde(default)]
        selector: Option<String>,
        /// Rhai merge script for Merge strategy
        #[serde(default)]
        merge_script: Option<String>,
    },

    /// Conditional router
    Router {
        routes: Vec<RouteDefinition>,
    },

    /// Data transformer (Rhai script)
    Transform {
        /// Rhai script for data transformation
        script: String,
    },

    /// Passthrough node (no-op, useful for graph organization)
    Passthrough,
}

/// Output destination configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputDestination {
    /// Send to originating WebSocket client
    #[default]
    WebSocket,
    /// Send to LiveKit room
    LiveKit,
    /// Send to specific endpoint node
    Endpoint {
        node_id: String,
    },
    /// Broadcast to all connected clients
    Broadcast,
    /// Discard output (useful for side effects only)
    Discard,
}

/// HTTP method enum
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    #[default]
    POST,
    GET,
    PUT,
    PATCH,
    DELETE,
}

impl From<HttpMethod> for reqwest::Method {
    fn from(method: HttpMethod) -> Self {
        match method {
            HttpMethod::POST => reqwest::Method::POST,
            HttpMethod::GET => reqwest::Method::GET,
            HttpMethod::PUT => reqwest::Method::PUT,
            HttpMethod::PATCH => reqwest::Method::PATCH,
            HttpMethod::DELETE => reqwest::Method::DELETE,
        }
    }
}

/// Join strategy for aggregating parallel branch results
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JoinStrategy {
    /// Return first completed result
    #[default]
    First,
    /// Wait for all, return array
    All,
    /// Select by expression (requires selector field)
    Best,
    /// Merge results (requires merge_script field)
    Merge,
}

/// Route definition for conditional routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteDefinition {
    /// Rhai condition expression (evaluated to bool)
    #[serde(default)]
    pub condition: Option<String>,

    /// Target node ID if condition matches
    pub target: String,

    /// Priority (higher = evaluated first)
    #[serde(default)]
    pub priority: i32,

    /// Mark as default route (used if no conditions match)
    #[serde(default)]
    pub default: bool,
}

impl RouteDefinition {
    /// Create a new route definition
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            condition: None,
            target: target.into(),
            priority: 0,
            default: false,
        }
    }

    /// Add a condition to this route
    pub fn with_condition(mut self, condition: impl Into<String>) -> Self {
        self.condition = Some(condition.into());
        self
    }

    /// Mark this as the default route
    pub fn as_default(mut self) -> Self {
        self.default = true;
        self
    }
}

/// Edge definition (connection between nodes)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeDefinition {
    /// Source node ID
    pub from: String,

    /// Target node ID
    pub to: String,

    /// Optional condition for this edge (Rhai expression)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,

    /// Simple switch pattern (alternative to condition)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub switch: Option<SwitchPattern>,

    /// Edge priority (higher = preferred when multiple edges match)
    #[serde(default)]
    pub priority: i32,

    /// Ring buffer capacity (samples)
    #[serde(default = "default_buffer_capacity")]
    pub buffer_capacity: usize,

    /// Transform data before passing to target (Rhai script)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transform: Option<String>,
}

impl EdgeDefinition {
    /// Create a new edge definition
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            condition: None,
            switch: None,
            priority: 0,
            buffer_capacity: default_buffer_capacity(),
            transform: None,
        }
    }

    /// Add a condition to this edge
    pub fn with_condition(mut self, condition: impl Into<String>) -> Self {
        self.condition = Some(condition.into());
        self
    }

    /// Add a switch pattern to this edge
    pub fn with_switch(mut self, switch: SwitchPattern) -> Self {
        self.switch = Some(switch);
        self
    }

    /// Set edge priority
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }
}

/// Switch pattern for simple field-based routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwitchPattern {
    /// Field path to match against (dot-separated, e.g., "stt_result.is_final")
    pub field: String,

    /// Match patterns: value -> target_node
    pub cases: HashMap<String, String>,

    /// Default target if no case matches
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

impl SwitchPattern {
    /// Create a new switch pattern
    pub fn new(field: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            cases: HashMap::new(),
            default: None,
        }
    }

    /// Add a case to the switch pattern
    pub fn add_case(mut self, value: impl Into<String>, target: impl Into<String>) -> Self {
        self.cases.insert(value.into(), target.into());
        self
    }

    /// Set the default target
    pub fn with_default(mut self, target: impl Into<String>) -> Self {
        self.default = Some(target.into());
        self
    }

    /// Get the field path as a vector of segments
    pub fn field_segments(&self) -> Vec<&str> {
        self.field.split('.').collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dag_definition_builder() {
        let mut dag = DAGDefinition::new("test-dag", "Test DAG");
        dag.add_node(NodeDefinition::new("input", NodeType::AudioInput));
        dag.add_node(NodeDefinition::new("stt", NodeType::SttProvider {
            provider: "deepgram".to_string(),
            model: None,
            language: None,
        }));
        dag.add_edge(EdgeDefinition::new("input", "stt"));
        dag.with_entry("input");
        dag.add_exit("stt");

        assert_eq!(dag.nodes.len(), 2);
        assert_eq!(dag.edges.len(), 1);
        assert_eq!(dag.entry_node, "input");
        assert_eq!(dag.exit_nodes, vec!["stt"]);
    }

    #[test]
    fn test_dag_validation() {
        let mut dag = DAGDefinition::new("test-dag", "Test DAG");
        dag.add_node(NodeDefinition::new("input", NodeType::AudioInput));
        dag.entry_node = "input".to_string();
        dag.exit_nodes = vec!["input".to_string()];

        assert!(dag.validate_structure().is_ok());

        // Test missing entry node
        dag.entry_node = "missing".to_string();
        let errors = dag.validate_structure().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("missing")));
    }

    #[test]
    fn test_node_type_serialization() {
        let node = NodeDefinition::new("stt", NodeType::SttProvider {
            provider: "deepgram".to_string(),
            model: Some("nova-2".to_string()),
            language: Some("en-US".to_string()),
        });

        let json = serde_json::to_string(&node).unwrap();
        assert!(json.contains("stt_provider"));
        assert!(json.contains("deepgram"));

        let parsed: NodeDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "stt");
    }

    #[test]
    fn test_edge_definition() {
        let edge = EdgeDefinition::new("input", "stt")
            .with_condition("is_speech_final == true")
            .with_priority(10);

        assert_eq!(edge.from, "input");
        assert_eq!(edge.to, "stt");
        assert!(edge.condition.is_some());
        assert_eq!(edge.priority, 10);
    }

    #[test]
    fn test_switch_pattern() {
        let switch = SwitchPattern::new("stt_result.language")
            .add_case("en-US", "english_handler")
            .add_case("es-ES", "spanish_handler")
            .with_default("default_handler");

        assert_eq!(switch.field, "stt_result.language");
        assert_eq!(switch.cases.len(), 2);
        assert!(switch.default.is_some());
        assert_eq!(switch.field_segments(), vec!["stt_result", "language"]);
    }

    #[test]
    fn test_join_strategy_serialization() {
        let json = serde_json::to_string(&JoinStrategy::Best).unwrap();
        assert_eq!(json, "\"best\"");

        let parsed: JoinStrategy = serde_json::from_str("\"all\"").unwrap();
        assert_eq!(parsed, JoinStrategy::All);
    }

    #[test]
    fn test_yaml_deserialization() {
        let yaml = r#"
id: voice-bot
name: Voice Bot Pipeline
version: "1.0.0"
nodes:
  - id: input
    type: audio_input
  - id: stt
    type: stt_provider
    provider: deepgram
    model: nova-2
  - id: output
    type: text_output
    destination: web_socket
edges:
  - from: input
    to: stt
  - from: stt
    to: output
    condition: "is_final == true"
entry_node: input
exit_nodes:
  - output
"#;

        let dag: DAGDefinition = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(dag.id, "voice-bot");
        assert_eq!(dag.nodes.len(), 3);
        assert_eq!(dag.edges.len(), 2);
        assert!(dag.validate_structure().is_ok());
    }

    #[test]
    fn test_empty_dag_validation() {
        // Test completely empty DAG
        let dag = DAGDefinition::new("empty", "Empty DAG");
        let errors = dag.validate_structure().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("no nodes")));
        assert!(errors.iter().any(|e| e.contains("entry_node is empty")));
        assert!(errors.iter().any(|e| e.contains("no exit_nodes")));

        // Test DAG with nodes but empty entry_node
        let mut dag = DAGDefinition::new("test", "Test");
        dag.add_node(NodeDefinition::new("node1", NodeType::AudioInput));
        dag.exit_nodes = vec!["node1".to_string()];
        // entry_node is still empty
        let errors = dag.validate_structure().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("entry_node is empty")));

        // Test DAG with nodes but empty exit_nodes
        let mut dag = DAGDefinition::new("test", "Test");
        dag.add_node(NodeDefinition::new("node1", NodeType::AudioInput));
        dag.entry_node = "node1".to_string();
        // exit_nodes is still empty
        let errors = dag.validate_structure().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("no exit_nodes")));

        // Test DAG with empty node ID
        let mut dag = DAGDefinition::new("test", "Test");
        dag.add_node(NodeDefinition::new("", NodeType::AudioInput)); // empty ID
        dag.add_node(NodeDefinition::new("node1", NodeType::AudioInput));
        dag.entry_node = "node1".to_string();
        dag.exit_nodes = vec!["node1".to_string()];
        let errors = dag.validate_structure().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("empty ID")));
    }
}
