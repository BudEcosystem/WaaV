//! DAG compiler for validation and AST preparation
//!
//! This module handles compiling DAG definitions into executable form:
//! - Validates DAG structure (acyclicity, connectivity)
//! - Compiles Rhai expressions to AST
//! - Creates node instances

use std::collections::HashMap;
use std::sync::Arc;

use petgraph::Direction;
use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use rhai::Engine;
use tracing::{debug, info};

use super::definition::{DAGDefinition, EdgeDefinition, NodeDefinition, NodeType};
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

    /// Pre-computed split-branch plans, keyed by the split node index.
    ///
    /// For each split node this records, per declared branch, the branch entry
    /// node and the ordered set of "interior" nodes that are dominated by that
    /// branch entry (i.e. only reachable through it). The reconvergence/join
    /// node is NOT part of any branch interior — it executes once in the main
    /// topological sweep. This is what makes the sweep split-aware so each node
    /// executes exactly once (W-O3 bug #1) while branches still run in parallel.
    pub split_plans: HashMap<NodeIndex, Vec<SplitBranchPlan>>,

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

/// Execution plan for a single branch of a Split node.
///
/// Computed at compile time so the executor never has to re-derive branch
/// membership at runtime.
#[derive(Debug, Clone)]
pub struct SplitBranchPlan {
    /// The branch entry node ID (as declared on the Split node).
    pub branch_id: String,
    /// The branch entry node index.
    pub entry: NodeIndex,
    /// All interior nodes of this branch in topological order, including the
    /// entry node. These are nodes dominated by `entry` relative to the split:
    /// every path from the split to such a node passes through `entry`, and the
    /// node is not a reconvergence point shared with another branch.
    pub interior: Vec<NodeIndex>,
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
        definition
            .validate_structure()
            .map_err(|errors| DAGError::InvalidStructure(errors.join("; ")))?;

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
            let from_idx = *node_index
                .get(&edge_def.from)
                .ok_or_else(|| DAGError::UnknownNode(edge_def.from.clone()))?;
            let to_idx = *node_index
                .get(&edge_def.to)
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
        let entry = *node_index
            .get(&definition.entry_node)
            .ok_or_else(|| DAGError::EntryNodeNotFound(definition.entry_node.clone()))?;

        let exits: Vec<NodeIndex> = definition
            .exit_nodes
            .iter()
            .map(|id| {
                node_index
                    .get(id)
                    .ok_or_else(|| DAGError::ExitNodeNotFound(id.clone())).copied()
            })
            .collect::<DAGResult<Vec<_>>>()?;

        // Compile API key routes
        let api_key_routes: HashMap<String, NodeIndex> = definition
            .api_key_routes
            .iter()
            .filter_map(|(key, node_id)| node_index.get(node_id).map(|idx| (key.clone(), *idx)))
            .collect();

        // Pre-compute split-branch plans (W-O3 bug #1/#2). For every split node,
        // derive the interior nodes of each branch so the executor can run each
        // branch exactly once and let the join execute once in the main sweep.
        let split_plans = compute_split_plans(&graph, &topo_order);

        info!(
            dag_id = %definition.id,
            topo_order_len = %topo_order.len(),
            split_count = %split_plans.len(),
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
            split_plans,
            rhai_engine: self.rhai_engine.clone(),
            evaluator: self.evaluator.clone(),
            metrics: Arc::new(DAGMetrics::new()),
        })
    }

    /// Compile a single node definition
    fn compile_node(&self, def: &NodeDefinition) -> DAGResult<CompiledNode> {
        debug!(node_id = %def.id, node_type = ?def.node_type, "Compiling node");

        let node: Arc<dyn DAGNode> = match &def.node_type {
            NodeType::AudioInput => Arc::new(AudioInputNode::new(&def.id)),
            NodeType::TextInput => Arc::new(TextInputNode::new(&def.id)),
            NodeType::AudioOutput { destination } => {
                Arc::new(AudioOutputNode::new(&def.id, destination.clone()))
            }
            NodeType::TextOutput { destination } => {
                Arc::new(TextOutputNode::new(&def.id, destination.clone()))
            }
            NodeType::SttProvider {
                provider,
                model,
                language,
            } => {
                // Thread the node's `config` blob so the provider credential (`"api_key"`,
                // literal or `${ENV_VAR}`) reaches the STT config at execute time.
                let mut node =
                    STTProviderNode::new(&def.id, provider).with_config(def.config.clone());
                if let Some(m) = model {
                    node = node.with_model(m);
                }
                if let Some(l) = language {
                    node = node.with_language(l);
                }
                Arc::new(node)
            }
            NodeType::TtsProvider {
                provider,
                voice_id,
                model,
            } => {
                let mut node =
                    TTSProviderNode::new(&def.id, provider).with_config(def.config.clone());
                if let Some(v) = voice_id {
                    node = node.with_voice(v);
                }
                if let Some(m) = model {
                    node = node.with_model(m);
                }
                Arc::new(node)
            }
            NodeType::RealtimeProvider { provider, model } => {
                let mut node =
                    RealtimeProviderNode::new(&def.id, provider).with_config(def.config.clone());
                if let Some(m) = model {
                    node = node.with_model(m);
                }
                Arc::new(node)
            }
            NodeType::Processor { plugin } => Arc::new(ProcessorNode::new(&def.id, plugin)),
            NodeType::HttpEndpoint {
                url,
                method,
                headers,
                timeout_ms,
            } => {
                // Use try_new() for SSRF protection
                let mut node = HttpEndpointNode::try_new(&def.id, url)?.with_method(method.clone());
                for (key, value) in headers {
                    node = node.with_header(key, value);
                }
                if let Some(timeout) = timeout_ms {
                    node = node.with_timeout_ms(*timeout);
                }
                Arc::new(node)
            }
            NodeType::GrpcEndpoint {
                address,
                service,
                method,
                timeout_ms,
            } => {
                // Use try_new() for SSRF protection on client-supplied addresses.
                let mut node = GrpcEndpointNode::try_new(&def.id, address, service, method)?;
                if let Some(timeout) = timeout_ms {
                    node = node.with_timeout_ms(*timeout);
                }
                Arc::new(node)
            }
            NodeType::WebSocketEndpoint { url, headers } => {
                // Use try_new() for SSRF protection
                let mut node = WebSocketEndpointNode::try_new(&def.id, url)?;
                for (key, value) in headers {
                    node = node.with_header(key, value);
                }
                Arc::new(node)
            }
            NodeType::IpcEndpoint {
                shm_name,
                input_format,
                output_format,
            } => {
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
            NodeType::LlmEndpoint {
                base_url,
                model,
                api_key,
                system_prompt,
                temperature,
                max_tokens,
                streaming,
                tools,
                assistant_id,
                timeout_ms,
                headers,
            } => {
                // Build LLM config from node definition
                let mut llm_config = LlmEndpointConfig {
                    base_url: base_url.clone(),
                    model: model.clone(),
                    api_key: api_key.clone(),
                    system_prompt: system_prompt.clone(),
                    temperature: *temperature,
                    max_tokens: *max_tokens,
                    streaming: *streaming,
                    timeout_ms: timeout_ms.unwrap_or(60000),
                    headers: headers.clone(),
                    assistant_id: assistant_id.clone(),
                    ..Default::default()
                };

                // Parse tools from JSON if provided
                if let Some(tools_json) = tools
                    && let Ok(parsed_tools) = serde_json::from_value::<
                        Vec<super::nodes::ToolDefinition>,
                    >(tools_json.clone())
                    {
                        llm_config.tools = Some(parsed_tools);
                    }

                // Use try_new() for SSRF protection on the client-supplied base_url.
                Arc::new(LlmEndpointNode::try_new(&def.id, llm_config)?)
            }
            NodeType::WebhookOutput { url, headers } => {
                // Use try_new() for SSRF protection (S6): webhook URLs are client-supplied.
                let mut node = WebhookOutputNode::try_new(&def.id, url)?;
                for (key, value) in headers {
                    node = node.with_header(key, value);
                }
                Arc::new(node)
            }
            NodeType::Split { branches } => Arc::new(SplitNode::new(&def.id, branches.clone())),
            NodeType::Join {
                sources,
                strategy,
                selector,
                merge_script,
            } => {
                let mut node = JoinNode::new(&def.id, sources.clone(), *strategy);
                if let Some(s) = selector {
                    // Validate the selector compiles at compile time (W-O3 bug #4)
                    // rather than only failing per-execution. Selectors may use
                    // looping (array iteration), so allow it for the syntax check.
                    let mut check_engine = create_rhai_engine();
                    check_engine.set_allow_looping(true);
                    check_engine.compile(s).map_err(|e| {
                        DAGError::ExpressionCompilationError {
                            expression: s.clone(),
                            error: format!("Join '{}' selector: {}", def.id, e),
                        }
                    })?;
                    node = node.with_selector(s);
                }
                if let Some(m) = merge_script {
                    // Validate the merge script compiles at compile time.
                    let mut check_engine = create_rhai_engine();
                    check_engine.set_allow_looping(true);
                    check_engine.compile(m).map_err(|e| {
                        DAGError::ExpressionCompilationError {
                            expression: m.clone(),
                            error: format!("Join '{}' merge_script: {}", def.id, e),
                        }
                    })?;
                    node = node.with_merge_script(m);
                }
                Arc::new(node)
            }
            NodeType::Router { routes } => Arc::new(RouterNode::from_definitions(
                &def.id,
                routes.clone(),
                &self.evaluator,
            )?),
            NodeType::Transform { script } => Arc::new(TransformNode::compiled(&def.id, script)?),
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

/// Compute, for every split node, the per-branch interior node sets.
///
/// A node `n` is in the interior of a branch entered at `entry` (a direct
/// successor of the split) when **every** path from the split to `n` passes
/// through `entry`. Operationally, walking the graph in topological order, a
/// node is interior iff all of its predecessors are themselves interior nodes
/// of the same branch (the entry being interior by definition). This naturally
/// excludes reconvergence/join nodes, which have at least one predecessor from a
/// different branch and therefore execute once in the main sweep rather than per
/// branch. This is the data needed for single-execution split handling.
fn compute_split_plans(
    graph: &DiGraph<CompiledNode, CompiledEdge>,
    topo_order: &[NodeIndex],
) -> HashMap<NodeIndex, Vec<SplitBranchPlan>> {
    use std::collections::HashSet;

    let mut plans: HashMap<NodeIndex, Vec<SplitBranchPlan>> = HashMap::new();

    // Position of each node in the topological order, for ordered interiors.
    let mut topo_pos: HashMap<NodeIndex, usize> = HashMap::new();
    for (pos, &idx) in topo_order.iter().enumerate() {
        topo_pos.insert(idx, pos);
    }

    for &split_idx in topo_order {
        if graph[split_idx].node.node_type() != "split" {
            continue;
        }

        // Direct successors of the split are the branch entries.
        let branch_entries: Vec<NodeIndex> = graph
            .edges_directed(split_idx, Direction::Outgoing)
            .map(|e| e.target())
            .collect();

        // De-duplicate while preserving order (a branch may be listed once).
        let mut seen_entries = HashSet::new();
        let branch_entries: Vec<NodeIndex> = branch_entries
            .into_iter()
            .filter(|idx| seen_entries.insert(*idx))
            .collect();

        let mut branch_plans = Vec::with_capacity(branch_entries.len());

        for entry in branch_entries {
            // Compute the interior set for this branch entry.
            let mut interior: HashSet<NodeIndex> = HashSet::new();
            interior.insert(entry);

            // Walk forward in topological order from the entry. A node joins the
            // interior iff all its predecessors are already in this interior.
            let entry_pos = *topo_pos.get(&entry).unwrap_or(&0);
            for &candidate in &topo_order[entry_pos..] {
                if candidate == entry {
                    continue;
                }
                // Only consider nodes reachable from the entry through the interior:
                // require at least one predecessor in the interior.
                let preds: Vec<NodeIndex> = graph
                    .edges_directed(candidate, Direction::Incoming)
                    .map(|e| e.source())
                    .collect();
                if preds.is_empty() {
                    continue;
                }
                let has_interior_pred = preds.iter().any(|p| interior.contains(p));
                let all_preds_interior = preds.iter().all(|p| interior.contains(p));
                if has_interior_pred && all_preds_interior {
                    interior.insert(candidate);
                }
            }

            // Emit the interior in topological order.
            let mut ordered: Vec<NodeIndex> = interior.into_iter().collect();
            ordered.sort_by_key(|idx| topo_pos.get(idx).copied().unwrap_or(usize::MAX));

            branch_plans.push(SplitBranchPlan {
                branch_id: graph[entry].id.clone(),
                entry,
                interior: ordered,
            });
        }

        plans.insert(split_idx, branch_plans);
    }

    plans
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

        // Prefix match. `api_key_routes` is a HashMap whose iteration order is
        // randomized per instance, so when multiple patterns match (e.g. "team-"
        // and "team-eu-" for key "team-eu-7") a bare iteration would pick an
        // arbitrary one — nondeterministic routing across restarts. Sort the
        // candidates by (pattern length DESC, then lexicographic) so the MOST
        // SPECIFIC pattern always wins, deterministically.
        let mut patterns: Vec<(&str, NodeIndex)> = self
            .api_key_routes
            .iter()
            .map(|(pattern, idx)| (pattern.as_str(), *idx))
            .collect();
        patterns.sort_by(|(a, _), (b, _)| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));

        patterns
            .into_iter()
            .find(|(pattern, _)| api_key_id.starts_with(pattern))
            .map(|(_, idx)| idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::definition::OutputDestination;

    fn create_simple_dag() -> DAGDefinition {
        let mut dag = DAGDefinition::new("test-dag", "Test DAG");
        dag.add_node(NodeDefinition::new("input", NodeType::AudioInput));
        dag.add_node(NodeDefinition::new(
            "stt",
            NodeType::SttProvider {
                provider: "deepgram".to_string(),
                model: None,
                language: None,
            },
        ));
        dag.add_node(NodeDefinition::new(
            "output",
            NodeType::TextOutput {
                destination: OutputDestination::WebSocket,
            },
        ));
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
        dag.add_node(NodeDefinition::new(
            "output",
            NodeType::TextOutput {
                destination: OutputDestination::WebSocket,
            },
        ));
        dag.add_edge(EdgeDefinition::new("input", "output").with_condition("is_final == true"));
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
        dag.api_key_routes
            .insert("tenant_a".to_string(), "stt".to_string());

        let compiled = compiler.compile(dag).unwrap();

        assert!(compiled.get_api_key_route("tenant_a").is_some());
        assert!(compiled.get_api_key_route("tenant_b").is_none());
    }

    /// MASTER_PLAN §5 (router determinism): with OVERLAPPING prefix patterns the
    /// MOST SPECIFIC (longest) one must win, and the choice must be deterministic.
    /// Each iteration compiles a FRESH DAG — and thus a fresh `HashMap` with its
    /// own randomized iteration order — so pre-fix this flips routes between
    /// iterations; post-fix all 50 iterations must agree.
    #[test]
    fn test_api_key_route_most_specific_pattern_wins_deterministically() {
        for i in 0..50 {
            let compiler = DAGCompiler::new();
            let mut dag = DAGDefinition::new("route-dag", "Route DAG");
            dag.add_node(NodeDefinition::new("input", NodeType::TextInput));
            dag.add_node(NodeDefinition::new("generic", NodeType::Passthrough));
            dag.add_node(NodeDefinition::new("eu", NodeType::Passthrough));
            dag.add_node(NodeDefinition::new(
                "output",
                NodeType::TextOutput {
                    destination: OutputDestination::WebSocket,
                },
            ));
            dag.add_edge(EdgeDefinition::new("input", "generic"));
            dag.add_edge(EdgeDefinition::new("input", "eu"));
            dag.add_edge(EdgeDefinition::new("generic", "output"));
            dag.add_edge(EdgeDefinition::new("eu", "output"));
            dag.with_entry("input");
            dag.add_exit("output");
            // Two overlapping prefix patterns: "team-eu-" is more specific.
            dag.api_key_routes
                .insert("team-".to_string(), "generic".to_string());
            dag.api_key_routes
                .insert("team-eu-".to_string(), "eu".to_string());

            let compiled = compiler.compile(dag).unwrap();
            let eu_idx = compiled.get_node_index("eu").unwrap();
            let generic_idx = compiled.get_node_index("generic").unwrap();

            // Both patterns match "team-eu-7"; the longer one must win — always.
            assert_eq!(
                compiled.get_api_key_route("team-eu-7"),
                Some(eu_idx),
                "iteration {i}: longest pattern must win for 'team-eu-7'"
            );
            // Only the short pattern matches here.
            assert_eq!(
                compiled.get_api_key_route("team-us-1"),
                Some(generic_idx),
                "iteration {i}: 'team-us-1' must route via 'team-'"
            );
            // No pattern matches.
            assert_eq!(compiled.get_api_key_route("acme-1"), None);
        }
    }

    // ── W-O3 bug #3: SSRF closure on LLM base_url and gRPC address ──────────

    /// A DAG whose LLM node points `base_url` at the cloud-metadata endpoint
    /// must be rejected at compile time.
    #[test]
    fn test_llm_base_url_ssrf_rejected_at_compile() {
        let compiler = DAGCompiler::new();
        let mut dag = DAGDefinition::new("ssrf-llm", "SSRF LLM");
        dag.add_node(NodeDefinition::new("input", NodeType::TextInput));
        dag.add_node(NodeDefinition::new(
            "llm",
            NodeType::LlmEndpoint {
                base_url: "http://169.254.169.254/latest/meta-data/".to_string(),
                model: "gpt-4o".to_string(),
                api_key: None,
                system_prompt: None,
                temperature: None,
                max_tokens: None,
                streaming: false,
                tools: None,
                assistant_id: None,
                timeout_ms: None,
                headers: HashMap::new(),
            },
        ));
        dag.add_node(NodeDefinition::new("output", NodeType::Passthrough));
        dag.add_edge(EdgeDefinition::new("input", "llm"));
        dag.add_edge(EdgeDefinition::new("llm", "output"));
        dag.with_entry("input");
        dag.add_exit("output");

        let result = compiler.compile(dag);
        assert!(
            result.is_err(),
            "LLM base_url pointing at cloud metadata must be rejected at compile"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(err, DAGError::ConfigError(_)),
            "expected ConfigError (SSRF), got {:?}",
            err
        );
    }

    /// A DAG whose gRPC node targets a private/RFC1918 address must be rejected
    /// at compile time.
    #[test]
    fn test_grpc_address_ssrf_rejected_at_compile() {
        let compiler = DAGCompiler::new();
        let mut dag = DAGDefinition::new("ssrf-grpc", "SSRF gRPC");
        dag.add_node(NodeDefinition::new("input", NodeType::TextInput));
        dag.add_node(NodeDefinition::new(
            "grpc",
            NodeType::GrpcEndpoint {
                address: "10.0.0.5:50051".to_string(),
                service: "my.Service".to_string(),
                method: "Call".to_string(),
                timeout_ms: None,
            },
        ));
        dag.add_node(NodeDefinition::new("output", NodeType::Passthrough));
        dag.add_edge(EdgeDefinition::new("input", "grpc"));
        dag.add_edge(EdgeDefinition::new("grpc", "output"));
        dag.with_entry("input");
        dag.add_exit("output");

        let result = compiler.compile(dag);
        assert!(
            result.is_err(),
            "gRPC address to a private IP must be rejected at compile"
        );
        assert!(matches!(result.unwrap_err(), DAGError::ConfigError(_)));
    }

    // ── W-O3 bug #4: control-flow target + reachability + Join script ──────

    /// A Split node that references a non-existent branch must fail compile.
    #[test]
    fn test_malformed_split_branch_target_rejected() {
        let compiler = DAGCompiler::new();
        let mut dag = DAGDefinition::new("bad-split", "Bad Split");
        dag.add_node(NodeDefinition::new("input", NodeType::TextInput));
        dag.add_node(NodeDefinition::new(
            "split",
            NodeType::Split {
                branches: vec!["ghost".into()], // not a real node
            },
        ));
        dag.add_node(NodeDefinition::new("output", NodeType::Passthrough));
        dag.add_edge(EdgeDefinition::new("input", "split"));
        dag.add_edge(EdgeDefinition::new("split", "output"));
        dag.with_entry("input");
        dag.add_exit("output");

        let result = compiler.compile(dag);
        assert!(result.is_err(), "split referencing unknown branch must fail");
        assert!(matches!(
            result.unwrap_err(),
            DAGError::InvalidStructure(_)
        ));
    }

    /// A Router route targeting a non-existent node must fail compile.
    #[test]
    fn test_malformed_router_target_rejected() {
        use crate::dag::definition::RouteDefinition;
        let compiler = DAGCompiler::new();
        let mut dag = DAGDefinition::new("bad-router", "Bad Router");
        dag.add_node(NodeDefinition::new("input", NodeType::TextInput));
        dag.add_node(NodeDefinition::new(
            "router",
            NodeType::Router {
                routes: vec![RouteDefinition::new("nowhere").as_default()],
            },
        ));
        dag.add_node(NodeDefinition::new("output", NodeType::Passthrough));
        dag.add_edge(EdgeDefinition::new("input", "router"));
        dag.add_edge(EdgeDefinition::new("router", "output"));
        dag.with_entry("input");
        dag.add_exit("output");

        let result = compiler.compile(dag);
        assert!(result.is_err(), "router referencing unknown target must fail");
        assert!(matches!(
            result.unwrap_err(),
            DAGError::InvalidStructure(_)
        ));
    }

    /// A node unreachable from the entry must fail compile.
    #[test]
    fn test_unreachable_node_rejected() {
        let compiler = DAGCompiler::new();
        let mut dag = DAGDefinition::new("unreachable", "Unreachable");
        dag.add_node(NodeDefinition::new("input", NodeType::TextInput));
        dag.add_node(NodeDefinition::new("output", NodeType::Passthrough));
        // `orphan` has no incoming path from entry.
        dag.add_node(NodeDefinition::new("orphan", NodeType::Passthrough));
        dag.add_edge(EdgeDefinition::new("input", "output"));
        dag.with_entry("input");
        dag.add_exit("output");

        let result = compiler.compile(dag);
        assert!(result.is_err(), "unreachable node must fail compile");
        assert!(matches!(
            result.unwrap_err(),
            DAGError::InvalidStructure(_)
        ));
    }

    /// A Join node whose merge_script has a syntax error must fail compile
    /// (not only at execution time).
    #[test]
    fn test_bad_join_merge_script_rejected_at_compile() {
        use crate::dag::definition::JoinStrategy;
        let compiler = DAGCompiler::new();
        let mut dag = DAGDefinition::new("bad-join", "Bad Join");
        dag.add_node(NodeDefinition::new("input", NodeType::TextInput));
        dag.add_node(NodeDefinition::new("a", NodeType::Passthrough));
        dag.add_node(NodeDefinition::new("b", NodeType::Passthrough));
        dag.add_node(NodeDefinition::new(
            "split",
            NodeType::Split {
                branches: vec!["a".into(), "b".into()],
            },
        ));
        dag.add_node(NodeDefinition::new(
            "join",
            NodeType::Join {
                sources: vec!["a".into(), "b".into()],
                strategy: JoinStrategy::Merge,
                selector: None,
                // Deliberately broken Rhai (unterminated).
                merge_script: Some("let x = ;;; @@@ (".to_string()),
            },
        ));
        dag.add_node(NodeDefinition::new("output", NodeType::Passthrough));
        dag.add_edge(EdgeDefinition::new("input", "split"));
        dag.add_edge(EdgeDefinition::new("split", "a"));
        dag.add_edge(EdgeDefinition::new("split", "b"));
        dag.add_edge(EdgeDefinition::new("a", "join"));
        dag.add_edge(EdgeDefinition::new("b", "join"));
        dag.add_edge(EdgeDefinition::new("join", "output"));
        dag.with_entry("input");
        dag.add_exit("output");

        let result = compiler.compile(dag);
        assert!(
            result.is_err(),
            "join with invalid merge_script must fail at compile"
        );
        assert!(matches!(
            result.unwrap_err(),
            DAGError::ExpressionCompilationError { .. }
        ));
    }

    /// A public LLM base_url must still compile (no false positive).
    #[test]
    fn test_llm_public_base_url_compiles() {
        let compiler = DAGCompiler::new();
        let mut dag = DAGDefinition::new("ok-llm", "OK LLM");
        dag.add_node(NodeDefinition::new("input", NodeType::TextInput));
        dag.add_node(NodeDefinition::new(
            "llm",
            NodeType::LlmEndpoint {
                base_url: "https://api.openai.com/v1".to_string(),
                model: "gpt-4o".to_string(),
                api_key: None,
                system_prompt: None,
                temperature: None,
                max_tokens: None,
                streaming: false,
                tools: None,
                assistant_id: None,
                timeout_ms: None,
                headers: HashMap::new(),
            },
        ));
        dag.add_node(NodeDefinition::new("output", NodeType::Passthrough));
        dag.add_edge(EdgeDefinition::new("input", "llm"));
        dag.add_edge(EdgeDefinition::new("llm", "output"));
        dag.with_entry("input");
        dag.add_exit("output");

        assert!(
            compiler.compile(dag).is_ok(),
            "public LLM base_url should compile"
        );
    }
}
