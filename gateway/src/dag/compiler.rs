//! DAG compiler for validation and AST preparation
//!
//! This module handles compiling DAG definitions into executable form:
//! - Validates DAG structure (acyclicity, connectivity)
//! - Compiles Rhai expressions to AST
//! - Creates node instances

use std::collections::HashMap;
use std::sync::Arc;

use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use rhai::Engine;
use tracing::{debug, info};

use super::definition::{DAGDefinition, NodeDefinition, EdgeDefinition, NodeType};
use super::edges::CompiledEdge;
use super::error::{DAGError, DAGResult};
use super::metrics::DAGMetrics;
use super::nodes::*;
use super::routing::{ConditionEvaluator, create_rhai_engine};

/// Compiled DAG ready for execution
pub struct CompiledDAG {
    /// The DAG definition
    pub definition: DAGDefinition,

    /// Graph structure
    pub graph: DiGraph<CompiledNode, CompiledEdge>,

    /// Node ID to graph index mapping
    pub node_index: HashMap<String, NodeIndex>,

    /// Pre-computed topological order
    pub topo_order: Vec<NodeIndex>,

    /// Entry node index
    pub entry: NodeIndex,

    /// Exit node indices
    pub exits: Vec<NodeIndex>,

    /// API key routing table
    pub api_key_routes: HashMap<String, NodeIndex>,

    /// Rhai engine for expression evaluation
    pub rhai_engine: Arc<Engine>,

    /// Condition evaluator
    pub evaluator: Arc<ConditionEvaluator>,

    /// Metrics collector
    pub metrics: Arc<DAGMetrics>,
}

impl std::fmt::Debug for CompiledDAG {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledDAG")
            .field("id", &self.definition.id)
            .field("name", &self.definition.name)
            .field("nodes", &self.topo_order.len())
            .field("entry", &self.entry)
            .field("exits", &self.exits)
            .finish()
    }
}

/// Compiled node with pre-initialized resources
pub struct CompiledNode {
    /// Node ID
    pub id: String,
    /// Node implementation
    pub node: Arc<dyn DAGNode>,
    /// Original definition
    pub definition: NodeDefinition,
}

impl std::fmt::Debug for CompiledNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledNode")
            .field("id", &self.id)
            .field("type", &self.node.node_type())
            .finish()
    }
}

/// DAG compiler
pub struct DAGCompiler {
    rhai_engine: Arc<Engine>,
    evaluator: Arc<ConditionEvaluator>,
}

impl DAGCompiler {
    /// Create a new DAG compiler
    pub fn new() -> Self {
        let engine = Arc::new(create_rhai_engine());
        let evaluator = Arc::new(ConditionEvaluator::new(engine.clone()));

        Self {
            rhai_engine: engine,
            evaluator,
        }
    }

    /// Compile a DAG definition into an executable form
    pub fn compile(&self, definition: DAGDefinition) -> DAGResult<CompiledDAG> {
        info!(
            dag_id = %definition.id,
            dag_name = %definition.name,
            node_count = %definition.nodes.len(),
            edge_count = %definition.edges.len(),
            "Compiling DAG"
        );

        // Validate the definition structure
        definition.validate_structure().map_err(|errors| {
            DAGError::InvalidStructure(errors.join("; "))
        })?;

        // Build the graph
        let mut graph = DiGraph::new();
        let mut node_index = HashMap::new();

        // Add nodes to graph
        for node_def in &definition.nodes {
            let compiled_node = self.compile_node(node_def)?;
            let idx = graph.add_node(compiled_node);
            node_index.insert(node_def.id.clone(), idx);
        }

        // Add edges to graph
        for edge_def in &definition.edges {
            let from_idx = *node_index.get(&edge_def.from)
                .ok_or_else(|| DAGError::UnknownNode(edge_def.from.clone()))?;
            let to_idx = *node_index.get(&edge_def.to)
                .ok_or_else(|| DAGError::UnknownNode(edge_def.to.clone()))?;

            let compiled_edge = self.compile_edge(edge_def)?;
            graph.add_edge(from_idx, to_idx, compiled_edge);
        }

        // Compute topological order (validates acyclicity)
        let topo_order = toposort(&graph, None).map_err(|cycle| {
            let node = &graph[cycle.node_id()];
            DAGError::CycleDetected(node.id.clone())
        })?;

        // Get entry and exit nodes
        let entry = *node_index.get(&definition.entry_node)
            .ok_or_else(|| DAGError::EntryNodeNotFound(definition.entry_node.clone()))?;

        let exits: Vec<NodeIndex> = definition.exit_nodes.iter()
            .map(|id| node_index.get(id)
                .ok_or_else(|| DAGError::ExitNodeNotFound(id.clone()))
                .map(|idx| *idx))
            .collect::<DAGResult<Vec<_>>>()?;

        // Compile API key routes
        let api_key_routes: HashMap<String, NodeIndex> = definition.api_key_routes.iter()
            .filter_map(|(key, node_id)| {
                node_index.get(node_id).map(|idx| (key.clone(), *idx))
            })
            .collect();

        info!(
            dag_id = %definition.id,
            topo_order_len = %topo_order.len(),
            "DAG compiled successfully"
        );

        Ok(CompiledDAG {
            definition,
            graph,
            node_index,
            topo_order,
            entry,
            exits,
            api_key_routes,
            rhai_engine: self.rhai_engine.clone(),
            evaluator: self.evaluator.clone(),
            metrics: Arc::new(DAGMetrics::new()),
        })
    }

    /// Compile a single node definition
    fn compile_node(&self, def: &NodeDefinition) -> DAGResult<CompiledNode> {
        debug!(node_id = %def.id, node_type = ?def.node_type, "Compiling node");

        let node: Arc<dyn DAGNode> = match &def.node_type {
            NodeType::AudioInput => {
                Arc::new(AudioInputNode::new(&def.id))
            }
            NodeType::TextInput => {
                Arc::new(TextInputNode::new(&def.id))
            }
            NodeType::AudioOutput { destination } => {
                Arc::new(AudioOutputNode::new(&def.id, destination.clone()))
            }
            NodeType::TextOutput { destination } => {
                Arc::new(TextOutputNode::new(&def.id, destination.clone()))
            }
            NodeType::SttProvider { provider, model, language } => {
                let mut node = STTProviderNode::new(&def.id, provider);
                if let Some(m) = model {
                    node = node.with_model(m);
                }
                if let Some(l) = language {
                    node = node.with_language(l);
                }
                Arc::new(node)
            }
            NodeType::TtsProvider { provider, voice_id, model } => {
                let mut node = TTSProviderNode::new(&def.id, provider);
                if let Some(v) = voice_id {
                    node = node.with_voice(v);
                }
                if let Some(m) = model {
                    node = node.with_model(m);
                }
                Arc::new(node)
            }
            NodeType::RealtimeProvider { provider, model } => {
                let mut node = RealtimeProviderNode::new(&def.id, provider);
                if let Some(m) = model {
                    node = node.with_model(m);
                }
                Arc::new(node)
            }
            NodeType::Processor { plugin } => {
                Arc::new(ProcessorNode::new(&def.id, plugin))
            }
            NodeType::HttpEndpoint { url, method, headers, timeout_ms } => {
                let mut node = HttpEndpointNode::new(&def.id, url)
                    .with_method(method.clone());
                for (key, value) in headers {
                    node = node.with_header(key, value);
                }
                if let Some(timeout) = timeout_ms {
                    node = node.with_timeout_ms(*timeout);
                }
                Arc::new(node)
            }
            NodeType::GrpcEndpoint { address, service, method, timeout_ms } => {
                let mut node = GrpcEndpointNode::new(&def.id, address, service, method);
                if let Some(timeout) = timeout_ms {
                    node = node.with_timeout_ms(*timeout);
                }
                Arc::new(node)
            }
            NodeType::WebSocketEndpoint { url, headers } => {
                let mut node = WebSocketEndpointNode::new(&def.id, url);
                for (key, value) in headers {
                    node = node.with_header(key, value);
                }
                Arc::new(node)
            }
            NodeType::IpcEndpoint { shm_name, input_format, output_format } => {
                // Use try_new() for proper error handling with input validation
                let mut node = IpcEndpointNode::try_new(&def.id, shm_name)?;
                if let Some(f) = input_format {
                    node = node.with_input_format(f);
                }
                if let Some(f) = output_format {
                    node = node.with_output_format(f);
                }
                Arc::new(node)
            }
            NodeType::LiveKitEndpoint { room, track_type } => {
                let mut node = LiveKitEndpointNode::new(&def.id);
                if let Some(r) = room {
                    node = node.with_room(r);
                }
                if let Some(t) = track_type {
                    node = node.with_track_type(t);
                }
                Arc::new(node)
            }
            NodeType::WebhookOutput { url, headers } => {
                let mut node = WebhookOutputNode::new(&def.id, url);
                for (key, value) in headers {
                    node = node.with_header(key, value);
                }
                Arc::new(node)
            }
            NodeType::Split { branches } => {
                Arc::new(SplitNode::new(&def.id, branches.clone()))
            }
            NodeType::Join { sources, strategy, selector, merge_script } => {
                let mut node = JoinNode::new(&def.id, sources.clone(), *strategy);
                if let Some(s) = selector {
                    node = node.with_selector(s);
                }
                if let Some(m) = merge_script {
                    node = node.with_merge_script(m);
                }
                Arc::new(node)
            }
            NodeType::Router { routes } => {
                Arc::new(RouterNode::from_definitions(&def.id, routes.clone(), &self.evaluator)?)
            }
            NodeType::Transform { script } => {
                Arc::new(TransformNode::compiled(&def.id, script)?)
            }
            NodeType::Passthrough => {
                Arc::new(super::nodes::transform::PassthroughNode::new(&def.id))
            }
        };

        Ok(CompiledNode {
            id: def.id.clone(),
            node,
            definition: def.clone(),
        })
    }

    /// Compile a single edge definition
    fn compile_edge(&self, def: &EdgeDefinition) -> DAGResult<CompiledEdge> {
        debug!(from = %def.from, to = %def.to, "Compiling edge");

        let condition = if let Some(ref expr) = def.condition {
            Some(self.evaluator.compile_expression(expr)?)
        } else if let Some(ref switch) = def.switch {
            Some(self.evaluator.compile_switch(switch)?)
        } else {
            None
        };

        let transform = if let Some(ref script) = def.transform {
            let ast = self.rhai_engine.compile(script).map_err(|e| {
                DAGError::ExpressionCompilationError {
                    expression: script.clone(),
                    error: e.to_string(),
                }
            })?;
            Some(Arc::new(ast))
        } else {
            None
        };

        Ok(CompiledEdge {
            from: def.from.clone(),
            to: def.to.clone(),
            condition,
            priority: def.priority,
            transform,
        })
    }
}

impl Default for DAGCompiler {
    fn default() -> Self {
        Self::new()
    }
}

impl CompiledDAG {
    /// Get a node by ID
    pub fn get_node(&self, id: &str) -> Option<&CompiledNode> {
        self.node_index.get(id).map(|idx| &self.graph[*idx])
    }

    /// Get node index by ID
    pub fn get_node_index(&self, id: &str) -> Option<NodeIndex> {
        self.node_index.get(id).copied()
    }

    /// Get outgoing edges from a node
    pub fn outgoing_edges(&self, node_idx: NodeIndex) -> Vec<(NodeIndex, &CompiledEdge)> {
        self.graph
            .edges_directed(node_idx, Direction::Outgoing)
            .map(|edge| (edge.target(), edge.weight()))
            .collect()
    }

    /// Get incoming edges to a node
    pub fn incoming_edges(&self, node_idx: NodeIndex) -> Vec<(NodeIndex, &CompiledEdge)> {
        self.graph
            .edges_directed(node_idx, Direction::Incoming)
            .map(|edge| (edge.source(), edge.weight()))
            .collect()
    }

    /// Check if a node is an exit node
    pub fn is_exit_node(&self, node_idx: NodeIndex) -> bool {
        self.exits.contains(&node_idx)
    }

    /// Get API key route if available
    pub fn get_api_key_route(&self, api_key_id: &str) -> Option<NodeIndex> {
        // Try exact match first
        if let Some(idx) = self.api_key_routes.get(api_key_id) {
            return Some(*idx);
        }

        // Try prefix match
        for (pattern, idx) in &self.api_key_routes {
            if api_key_id.starts_with(pattern) {
                return Some(*idx);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::definition::OutputDestination;

    fn create_simple_dag() -> DAGDefinition {
        let mut dag = DAGDefinition::new("test-dag", "Test DAG");
        dag.add_node(NodeDefinition::new("input", NodeType::AudioInput));
        dag.add_node(NodeDefinition::new("stt", NodeType::SttProvider {
            provider: "deepgram".to_string(),
            model: None,
            language: None,
        }));
        dag.add_node(NodeDefinition::new("output", NodeType::TextOutput {
            destination: OutputDestination::WebSocket,
        }));
        dag.add_edge(EdgeDefinition::new("input", "stt"));
        dag.add_edge(EdgeDefinition::new("stt", "output"));
        dag.with_entry("input");
        dag.add_exit("output");
        dag
    }

    #[test]
    fn test_compile_simple_dag() {
        let compiler = DAGCompiler::new();
        let dag = create_simple_dag();

        let compiled = compiler.compile(dag).unwrap();

        assert_eq!(compiled.topo_order.len(), 3);
        assert!(compiled.get_node("input").is_some());
        assert!(compiled.get_node("stt").is_some());
        assert!(compiled.get_node("output").is_some());
    }

    #[test]
    fn test_compile_with_condition() {
        let compiler = DAGCompiler::new();
        let mut dag = DAGDefinition::new("test-dag", "Test DAG");
        dag.add_node(NodeDefinition::new("input", NodeType::TextInput));
        dag.add_node(NodeDefinition::new("output", NodeType::TextOutput {
            destination: OutputDestination::WebSocket,
        }));
        dag.add_edge(EdgeDefinition::new("input", "output")
            .with_condition("is_final == true"));
        dag.with_entry("input");
        dag.add_exit("output");

        let compiled = compiler.compile(dag).unwrap();
        assert_eq!(compiled.topo_order.len(), 2);
    }

    #[test]
    fn test_cycle_detection() {
        let compiler = DAGCompiler::new();
        let mut dag = DAGDefinition::new("test-dag", "Test DAG");
        dag.add_node(NodeDefinition::new("a", NodeType::Passthrough));
        dag.add_node(NodeDefinition::new("b", NodeType::Passthrough));
        dag.add_edge(EdgeDefinition::new("a", "b"));
        dag.add_edge(EdgeDefinition::new("b", "a")); // Cycle!
        dag.with_entry("a");
        dag.add_exit("b");

        let result = compiler.compile(dag);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DAGError::CycleDetected(_)));
    }

    #[test]
    fn test_unknown_node_reference() {
        let compiler = DAGCompiler::new();
        let mut dag = DAGDefinition::new("test-dag", "Test DAG");
        dag.add_node(NodeDefinition::new("input", NodeType::AudioInput));
        dag.add_edge(EdgeDefinition::new("input", "nonexistent"));
        dag.with_entry("input");
        dag.add_exit("nonexistent");

        let result = compiler.compile(dag);
        assert!(result.is_err());
    }

    #[test]
    fn test_api_key_routing() {
        let compiler = DAGCompiler::new();
        let mut dag = create_simple_dag();
        dag.api_key_routes.insert("tenant_a".to_string(), "stt".to_string());

        let compiled = compiler.compile(dag).unwrap();

        assert!(compiled.get_api_key_route("tenant_a").is_some());
        assert!(compiled.get_api_key_route("tenant_b").is_none());
    }
}
