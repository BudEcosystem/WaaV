//! DAG executor for pipeline execution
//!
//! This module handles executing compiled DAGs:
//! - Topological traversal of nodes
//! - Condition evaluation for edge routing
//! - Router-based dynamic routing
//! - Edge transform execution
//! - Parallel execution for split/join patterns
//! - Metrics collection

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use petgraph::graph::NodeIndex;
use rhai::{Dynamic, Scope};
use tracing::{debug, info, warn};

use super::compiler::CompiledDAG;
use super::context::DAGContext;
use super::error::{DAGError, DAGResult};
use super::nodes::DAGData;
use super::routing::create_rhai_engine;

/// DAG executor for running compiled pipelines
pub struct DAGExecutor {
    /// Default execution timeout
    default_timeout: Duration,
    /// Enable parallel execution for split nodes
    enable_parallelism: bool,
    /// Maximum concurrent branch executions
    max_concurrent_branches: usize,
}

impl DAGExecutor {
    /// Create a new DAG executor
    pub fn new() -> Self {
        Self {
            default_timeout: Duration::from_secs(30),
            enable_parallelism: true,
            max_concurrent_branches: 10,
        }
    }

    /// Set default execution timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Disable parallel execution
    pub fn without_parallelism(mut self) -> Self {
        self.enable_parallelism = false;
        self
    }

    /// Set maximum concurrent branches
    pub fn with_max_branches(mut self, max: usize) -> Self {
        self.max_concurrent_branches = max;
        self
    }

    /// Execute a DAG with the given input
    pub async fn execute(
        &self,
        dag: &CompiledDAG,
        input: DAGData,
        ctx: &mut DAGContext,
    ) -> DAGResult<DAGData> {
        let start_time = Instant::now();

        // Check for API key routing override
        let start_node = if let Some(api_key_id) = &ctx.api_key_id {
            dag.get_api_key_route(api_key_id).unwrap_or(dag.entry)
        } else {
            dag.entry
        };

        debug!(
            dag_id = %dag.definition.id,
            start_node = %dag.graph[start_node].id,
            input_type = %input.type_name(),
            "Starting DAG execution"
        );

        // Execute from start node
        let result = self.execute_from_node(dag, start_node, input, ctx).await;

        // Record metrics
        let duration = start_time.elapsed();
        match &result {
            Ok(_) => {
                dag.metrics.record_success(duration);
                info!(
                    dag_id = %dag.definition.id,
                    duration_ms = %duration.as_millis(),
                    "DAG execution completed successfully"
                );
            }
            Err(e) => {
                if matches!(e, DAGError::Cancelled) {
                    dag.metrics.record_cancellation();
                } else {
                    dag.metrics.record_failure(duration);
                }
                warn!(
                    dag_id = %dag.definition.id,
                    error = %e,
                    duration_ms = %duration.as_millis(),
                    "DAG execution failed"
                );
            }
        }

        result
    }

    /// Execute from a specific node following topological order
    ///
    /// This method handles:
    /// - Standard topological execution
    /// - Router-based dynamic routing (skips non-target paths)
    /// - Edge transform execution
    async fn execute_from_node(
        &self,
        dag: &CompiledDAG,
        start: NodeIndex,
        input: DAGData,
        ctx: &mut DAGContext,
    ) -> DAGResult<DAGData> {
        // Find position of start node in topo order
        let start_pos = dag.topo_order.iter()
            .position(|&n| n == start)
            .ok_or(DAGError::InvalidStartNode)?;

        // Track node outputs for downstream nodes
        let mut node_outputs: HashMap<NodeIndex, DAGData> = HashMap::new();

        // Track which nodes are reachable (for router-based pruning)
        // Initially, all nodes from start onwards are potentially reachable
        let mut reachable_nodes: HashSet<NodeIndex> = HashSet::new();
        for &node_idx in &dag.topo_order[start_pos..] {
            reachable_nodes.insert(node_idx);
        }

        // Store initial input for entry node
        node_outputs.insert(start, input);

        // Execute nodes in topological order starting from start_pos
        for &node_idx in &dag.topo_order[start_pos..] {
            // Check for cancellation
            if !ctx.should_continue() {
                return Err(if ctx.is_cancelled() {
                    DAGError::Cancelled
                } else {
                    DAGError::ExecutionTimeout(self.default_timeout.as_millis() as u64)
                });
            }

            // Skip nodes that are not reachable due to router pruning
            if !reachable_nodes.contains(&node_idx) {
                debug!(
                    node_id = %dag.graph[node_idx].id,
                    "Skipping unreachable node (router pruning)"
                );
                continue;
            }

            let compiled_node = &dag.graph[node_idx];

            // Gather input from predecessors (with edge transforms applied)
            let node_input = self.gather_inputs_with_transforms(dag, node_idx, &node_outputs, ctx).await?;

            // Skip if no input (conditional edge didn't match)
            if matches!(node_input, DAGData::Empty) && node_idx != start {
                continue;
            }

            // Execute the node
            let output = self.execute_node(dag, node_idx, node_input, ctx).await?;

            // Handle router node output - prune unreachable downstream paths
            if compiled_node.node.node_type() == "router" {
                if let Some(router_target) = ctx.metadata.get("router_target").cloned() {
                    // Find the target node index
                    if let Some(target_idx) = dag.get_node_index(&router_target) {
                        // Prune all outgoing paths except the target
                        self.prune_non_target_paths(
                            dag,
                            node_idx,
                            target_idx,
                            &mut reachable_nodes,
                        );
                        debug!(
                            router_id = %compiled_node.id,
                            target = %router_target,
                            "Router path pruning applied"
                        );
                    } else {
                        warn!(
                            router_id = %compiled_node.id,
                            target = %router_target,
                            "Router target node not found, continuing without pruning"
                        );
                    }
                    // Clear the router_target after processing
                    ctx.metadata.remove("router_target");
                }
            }

            // Store output for downstream nodes
            node_outputs.insert(node_idx, output);
        }

        // Collect outputs from exit nodes
        let mut exit_outputs: Vec<DAGData> = Vec::new();
        for &exit_idx in &dag.exits {
            if let Some(output) = node_outputs.remove(&exit_idx) {
                exit_outputs.push(output);
            }
        }

        // Return single output or multiple
        match exit_outputs.len() {
            0 => Ok(DAGData::Empty),
            1 => Ok(exit_outputs.remove(0)),
            _ => Ok(DAGData::Multiple(exit_outputs)),
        }
    }

    /// Prune nodes that are not on the path to the router target
    ///
    /// This marks nodes as unreachable if they are:
    /// 1. Direct successors of the router that are not the target
    /// 2. Descendants of those pruned nodes (unless also reachable via other paths)
    fn prune_non_target_paths(
        &self,
        dag: &CompiledDAG,
        router_idx: NodeIndex,
        target_idx: NodeIndex,
        reachable_nodes: &mut HashSet<NodeIndex>,
    ) {
        // Get all direct successors of the router
        let outgoing = dag.outgoing_edges(router_idx);

        // Collect nodes to prune (direct children that are not the target)
        let mut nodes_to_check: Vec<NodeIndex> = Vec::new();
        for (successor_idx, _edge) in outgoing {
            if successor_idx != target_idx {
                nodes_to_check.push(successor_idx);
            }
        }

        // For each non-target successor, check if it's ONLY reachable via the router
        // If so, prune it and its descendants
        for node_idx in nodes_to_check {
            // Check if this node has other incoming edges besides from the router
            let incoming = dag.incoming_edges(node_idx);
            let has_other_path = incoming.iter().any(|(source, _)| {
                *source != router_idx && reachable_nodes.contains(source)
            });

            if !has_other_path {
                // This node is only reachable via the router, prune it
                self.prune_subtree(dag, node_idx, target_idx, reachable_nodes);
            }
        }
    }

    /// Recursively prune a subtree of nodes
    ///
    /// Does not prune nodes that lead to the target or are reachable via other paths
    fn prune_subtree(
        &self,
        dag: &CompiledDAG,
        node_idx: NodeIndex,
        target_idx: NodeIndex,
        reachable_nodes: &mut HashSet<NodeIndex>,
    ) {
        // Don't prune the target itself
        if node_idx == target_idx {
            return;
        }

        // Don't prune if already pruned
        if !reachable_nodes.contains(&node_idx) {
            return;
        }

        // Check if any path from this node leads to the target
        if self.can_reach_target(dag, node_idx, target_idx, reachable_nodes) {
            return;
        }

        // Remove from reachable set
        reachable_nodes.remove(&node_idx);

        // Recursively check children
        let outgoing = dag.outgoing_edges(node_idx);
        for (child_idx, _edge) in outgoing {
            // Check if child has other reachable incoming edges
            let incoming = dag.incoming_edges(child_idx);
            let has_other_path = incoming.iter().any(|(source, _)| {
                *source != node_idx && reachable_nodes.contains(source)
            });

            if !has_other_path {
                self.prune_subtree(dag, child_idx, target_idx, reachable_nodes);
            }
        }
    }

    /// Check if a node can reach the target node
    fn can_reach_target(
        &self,
        dag: &CompiledDAG,
        start: NodeIndex,
        target: NodeIndex,
        reachable_nodes: &HashSet<NodeIndex>,
    ) -> bool {
        if start == target {
            return true;
        }

        let mut visited: HashSet<NodeIndex> = HashSet::new();
        let mut stack = vec![start];

        while let Some(node) = stack.pop() {
            if node == target {
                return true;
            }

            if visited.contains(&node) {
                continue;
            }
            visited.insert(node);

            // Only explore reachable nodes
            if !reachable_nodes.contains(&node) {
                continue;
            }

            for (child, _) in dag.outgoing_edges(node) {
                stack.push(child);
            }
        }

        false
    }

    /// Gather input from predecessor nodes with edge transform support
    ///
    /// This method:
    /// 1. Collects outputs from predecessor nodes
    /// 2. Evaluates edge conditions to determine which edges pass
    /// 3. Applies edge transforms (Rhai scripts) to transform data in transit
    async fn gather_inputs_with_transforms(
        &self,
        dag: &CompiledDAG,
        node_idx: NodeIndex,
        node_outputs: &HashMap<NodeIndex, DAGData>,
        ctx: &DAGContext,
    ) -> DAGResult<DAGData> {
        let incoming = dag.incoming_edges(node_idx);

        if incoming.is_empty() {
            // Entry node - return stored input
            return Ok(node_outputs.get(&node_idx).cloned().unwrap_or(DAGData::Empty));
        }

        // Collect inputs from all matching incoming edges
        let mut inputs: Vec<DAGData> = Vec::new();

        for (source_idx, edge) in incoming {
            // Check if source has output
            let source_output = match node_outputs.get(&source_idx) {
                Some(data) => data,
                None => continue, // Source hasn't executed yet
            };

            // Check edge condition
            let matches = if let Some(ref condition) = edge.condition {
                let data_json = source_output.to_json();
                dag.evaluator.evaluate(condition, &data_json, ctx)?
            } else {
                true // Unconditional edge
            };

            if matches {
                // Apply edge transform if present
                let transformed_data = if let Some(ref transform_ast) = edge.transform {
                    self.apply_edge_transform(source_output, transform_ast, ctx)?
                } else {
                    source_output.clone()
                };
                inputs.push(transformed_data);
            }
        }

        match inputs.len() {
            0 => Ok(DAGData::Empty),
            1 => Ok(inputs.remove(0)),
            _ => Ok(DAGData::Multiple(inputs)),
        }
    }

    /// Apply an edge transform script to data in transit
    ///
    /// The transform script receives the source data as `data` and should return
    /// the transformed data. The script has access to context variables like
    /// `stream_id`, `api_key`, etc.
    fn apply_edge_transform(
        &self,
        data: &DAGData,
        transform_ast: &rhai::AST,
        ctx: &DAGContext,
    ) -> DAGResult<DAGData> {
        // Create Rhai engine with security constraints
        let mut engine = create_rhai_engine();

        // Enable looping for transforms but with strict limits
        engine.set_allow_looping(true);
        engine.set_max_operations(1_000);
        engine.set_max_call_levels(16);

        // Create scope with data and context
        let mut scope = Scope::new();

        // Add the input data to scope
        scope.push("data", json_to_dynamic(&data.to_json()));
        scope.push_constant("stream_id", ctx.stream_id.clone());

        if let Some(api_key) = &ctx.api_key {
            scope.push_constant("api_key", api_key.clone());
        }
        if let Some(api_key_id) = &ctx.api_key_id {
            scope.push_constant("api_key_id", api_key_id.clone());
        }

        // Add metadata to scope
        for (key, value) in &ctx.metadata {
            scope.push_constant(key.clone(), value.clone());
        }

        // Execute the transform
        let result = engine.eval_ast_with_scope::<Dynamic>(&mut scope, transform_ast)
            .map_err(|e| DAGError::TransformError {
                edge: "edge transform".to_string(),
                error: e.to_string(),
            })?;

        // Convert result back to DAGData
        dynamic_to_dag_data(&result)
    }

    /// Legacy gather_inputs without transforms (for backward compatibility)
    #[allow(dead_code)]
    async fn gather_inputs(
        &self,
        dag: &CompiledDAG,
        node_idx: NodeIndex,
        node_outputs: &HashMap<NodeIndex, DAGData>,
        ctx: &DAGContext,
    ) -> DAGResult<DAGData> {
        self.gather_inputs_with_transforms(dag, node_idx, node_outputs, ctx).await
    }

    /// Execute a single node
    async fn execute_node(
        &self,
        dag: &CompiledDAG,
        node_idx: NodeIndex,
        input: DAGData,
        ctx: &mut DAGContext,
    ) -> DAGResult<DAGData> {
        let compiled_node = &dag.graph[node_idx];
        let node = &compiled_node.node;
        let node_id = &compiled_node.id;
        let node_type = node.node_type();

        debug!(
            node_id = %node_id,
            node_type = %node_type,
            input_type = %input.type_name(),
            "Executing node"
        );

        // Record start time
        ctx.record_node_start(node_id);

        let start = Instant::now();

        // Special handling for Split nodes - execute branches in parallel
        let result = if node_type == "split" {
            // First execute the split node to get branch info
            let split_result = node.execute(input.clone(), ctx).await?;

            // Get branches from context metadata (set by SplitNode)
            let branches: Vec<String> = ctx.metadata
                .get("split_branches")
                .map(|s| s.split(',').map(String::from).collect())
                .unwrap_or_default();

            if branches.is_empty() {
                warn!(node_id = %node_id, "Split node has no branches");
                Ok(split_result)
            } else {
                // Execute branches in parallel
                self.execute_split_branches(dag, &branches, input, ctx).await
            }
        } else {
            node.execute(input, ctx).await
        };

        let duration = start.elapsed();

        // Record metrics
        dag.metrics.record_node_execution(node_id, duration, result.is_ok());

        // Record end time
        ctx.record_node_end(node_id);

        match &result {
            Ok(output) => {
                debug!(
                    node_id = %node_id,
                    output_type = %output.type_name(),
                    duration_ms = %duration.as_millis(),
                    "Node executed successfully"
                );

                // Store result in context for expression evaluation
                ctx.set_node_result_arc(
                    node_id.clone(),
                    Arc::new(output.to_json()),
                );
            }
            Err(e) => {
                warn!(
                    node_id = %node_id,
                    error = %e,
                    duration_ms = %duration.as_millis(),
                    "Node execution failed"
                );
            }
        }

        result
    }

    /// Execute split branches in parallel
    ///
    /// Uses `Box::pin` to handle recursive async calls properly.
    fn execute_split_branches<'a>(
        &'a self,
        dag: &'a CompiledDAG,
        branches: &'a [String],
        input: DAGData,
        ctx: &'a DAGContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = DAGResult<DAGData>> + Send + 'a>> {
        Box::pin(async move {
            use futures::future::join_all;

            if !self.enable_parallelism || branches.len() <= 1 {
                // Sequential execution for single branch or disabled parallelism
                let mut results = Vec::with_capacity(branches.len());
                for branch_id in branches {
                    let branch_idx = dag.get_node_index(branch_id)
                        .ok_or_else(|| DAGError::UnknownNode(branch_id.clone()))?;

                    let mut branch_ctx = ctx.clone_for_branch();
                    // Use Box::pin for recursive call
                    let result = Box::pin(self.execute_from_node(dag, branch_idx, input.clone(), &mut branch_ctx)).await?;
                    results.push(result);
                }
                return Ok(DAGData::Multiple(results));
            }

            // Parallel execution using join_all
            // First, resolve all branch indices upfront
            let mut branch_indices = Vec::with_capacity(branches.len());
            for branch_id in branches {
                let branch_idx = dag.get_node_index(branch_id)
                    .ok_or_else(|| DAGError::UnknownNode(branch_id.clone()))?;
                branch_indices.push((branch_id.clone(), branch_idx));
            }

            // Create boxed futures for each branch
            let futures: Vec<_> = branch_indices
                .iter()
                .map(|(_, branch_idx)| {
                    let input_clone = input.clone();
                    let mut branch_ctx = ctx.clone_for_branch();
                    let branch_idx = *branch_idx;

                    // Create a boxed future for each branch
                    Box::pin(async move {
                        // Use Box::pin for recursive call
                        Box::pin(self.execute_from_node(dag, branch_idx, input_clone, &mut branch_ctx)).await
                    }) as std::pin::Pin<Box<dyn std::future::Future<Output = DAGResult<DAGData>> + Send>>
                })
                .collect();

            // Execute all branches concurrently
            let results: Vec<DAGResult<DAGData>> = join_all(futures).await;

            // Check for errors and collect results
            let mut collected_results = Vec::with_capacity(results.len());
            for (i, result) in results.into_iter().enumerate() {
                match result {
                    Ok(data) => collected_results.push(data),
                    Err(e) => {
                        let branch_id = branch_indices.get(i)
                            .map(|(id, _)| id.as_str())
                            .unwrap_or("unknown");
                        return Err(DAGError::SplitBranchError {
                            branch_id: branch_id.to_string(),
                            error: e.to_string(),
                        });
                    }
                }
            }

            Ok(DAGData::Multiple(collected_results))
        })
    }

    /// Execute split node (parallel branches) - public API
    ///
    /// Uses `Box::pin` to handle recursive async calls properly.
    /// Returns a vector of results from each branch.
    pub fn execute_split<'a>(
        &'a self,
        dag: &'a CompiledDAG,
        branches: &'a [String],
        input: DAGData,
        ctx: &'a DAGContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = DAGResult<Vec<DAGData>>> + Send + 'a>> {
        Box::pin(async move {
            use futures::future::join_all;

            if !self.enable_parallelism || branches.len() <= 1 {
                // Sequential execution
                let mut results = Vec::with_capacity(branches.len());
                for branch_id in branches {
                    let branch_idx = dag.get_node_index(branch_id)
                        .ok_or_else(|| DAGError::UnknownNode(branch_id.clone()))?;

                    let mut branch_ctx = ctx.clone_for_branch();
                    // Use Box::pin for recursive call
                    let result = Box::pin(self.execute_from_node(dag, branch_idx, input.clone(), &mut branch_ctx)).await?;
                    results.push(result);
                }
                return Ok(results);
            }

            // Parallel execution using join_all (avoids spawning and lifetime issues)
            // First, resolve all branch indices upfront
            let mut branch_indices = Vec::with_capacity(branches.len());
            for branch_id in branches {
                let branch_idx = dag.get_node_index(branch_id)
                    .ok_or_else(|| DAGError::UnknownNode(branch_id.clone()))?;
                branch_indices.push((branch_id.clone(), branch_idx));
            }

            // Create boxed futures for each branch
            let futures: Vec<_> = branch_indices
                .iter()
                .map(|(_, branch_idx)| {
                    let input_clone = input.clone();
                    let mut branch_ctx = ctx.clone_for_branch();
                    let branch_idx = *branch_idx;

                    // Create a boxed future for each branch
                    Box::pin(async move {
                        Box::pin(self.execute_from_node(dag, branch_idx, input_clone, &mut branch_ctx)).await
                    }) as std::pin::Pin<Box<dyn std::future::Future<Output = DAGResult<DAGData>> + Send>>
                })
                .collect();

            // Execute all branches concurrently
            let results: Vec<DAGResult<DAGData>> = join_all(futures).await;

            // Collect results and handle errors
            let mut collected_results = Vec::with_capacity(results.len());
            for (i, result) in results.into_iter().enumerate() {
                match result {
                    Ok(data) => collected_results.push(data),
                    Err(e) => {
                        let branch_id = branch_indices.get(i)
                            .map(|(id, _)| id.clone())
                            .unwrap_or_else(|| "unknown".to_string());
                        return Err(DAGError::SplitBranchError {
                            branch_id,
                            error: e.to_string(),
                        });
                    }
                }
            }

            Ok(collected_results)
        })
    }
}

impl Default for DAGExecutor {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper functions for Rhai Dynamic <-> JSON/DAGData conversion
// ─────────────────────────────────────────────────────────────────────────────

/// Convert serde_json::Value to Rhai Dynamic
fn json_to_dynamic(value: &serde_json::Value) -> Dynamic {
    use rhai::Array;

    match value {
        serde_json::Value::Null => Dynamic::UNIT,
        serde_json::Value::Bool(b) => Dynamic::from(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Dynamic::from(i)
            } else if let Some(f) = n.as_f64() {
                Dynamic::from(f)
            } else {
                Dynamic::UNIT
            }
        }
        serde_json::Value::String(s) => Dynamic::from(s.clone()),
        serde_json::Value::Array(arr) => {
            let rhai_arr: Array = arr.iter().map(json_to_dynamic).collect();
            Dynamic::from(rhai_arr)
        }
        serde_json::Value::Object(map) => {
            let mut rhai_map = rhai::Map::new();
            for (k, v) in map {
                rhai_map.insert(k.clone().into(), json_to_dynamic(v));
            }
            Dynamic::from(rhai_map)
        }
    }
}

/// Convert Rhai Dynamic to serde_json::Value
fn dynamic_to_json(value: &Dynamic) -> serde_json::Value {
    use rhai::Array;

    if value.is_unit() {
        return serde_json::Value::Null;
    }
    if let Some(b) = value.clone().try_cast::<bool>() {
        return serde_json::json!(b);
    }
    if let Some(i) = value.clone().try_cast::<i64>() {
        return serde_json::json!(i);
    }
    if let Some(f) = value.clone().try_cast::<f64>() {
        return serde_json::json!(f);
    }
    if let Some(s) = value.clone().try_cast::<String>() {
        return serde_json::json!(s);
    }
    if let Some(arr) = value.clone().try_cast::<Array>() {
        let json_arr: Vec<serde_json::Value> = arr.iter()
            .map(dynamic_to_json)
            .collect();
        return serde_json::json!(json_arr);
    }
    if let Some(map) = value.clone().try_cast::<rhai::Map>() {
        let mut json_map = serde_json::Map::new();
        for (k, v) in map.iter() {
            json_map.insert(k.to_string(), dynamic_to_json(v));
        }
        return serde_json::Value::Object(json_map);
    }
    // Fallback to string representation
    serde_json::json!(value.to_string())
}

/// Convert Rhai Dynamic to DAGData
fn dynamic_to_dag_data(value: &Dynamic) -> DAGResult<DAGData> {
    use bytes::Bytes;
    use rhai::Array;

    // Try specific types first
    if value.is_unit() {
        return Ok(DAGData::Empty);
    }
    if let Some(s) = value.clone().try_cast::<String>() {
        return Ok(DAGData::Text(s));
    }
    if let Some(b) = value.clone().try_cast::<bool>() {
        return Ok(DAGData::Json(serde_json::json!(b)));
    }
    if let Some(i) = value.clone().try_cast::<i64>() {
        return Ok(DAGData::Json(serde_json::json!(i)));
    }
    if let Some(f) = value.clone().try_cast::<f64>() {
        return Ok(DAGData::Json(serde_json::json!(f)));
    }
    if let Some(arr) = value.clone().try_cast::<Array>() {
        // Check if it looks like binary data (array of integers 0-255)
        if arr.iter().all(|v| {
            v.clone().try_cast::<i64>().map(|i| (0..=255).contains(&i)).unwrap_or(false)
        }) {
            let bytes: Vec<u8> = arr.iter()
                .filter_map(|v| v.clone().try_cast::<i64>().map(|i| i as u8))
                .collect();
            return Ok(DAGData::Binary(Bytes::from(bytes)));
        }

        // Otherwise treat as JSON array
        let json_arr: Vec<serde_json::Value> = arr.iter()
            .map(dynamic_to_json)
            .collect();
        return Ok(DAGData::Json(serde_json::json!(json_arr)));
    }
    if let Some(map) = value.clone().try_cast::<rhai::Map>() {
        return Ok(DAGData::Json(dynamic_to_json(&Dynamic::from(map))));
    }

    // Last resort: convert to JSON string
    Ok(DAGData::Json(serde_json::json!(value.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::compiler::DAGCompiler;
    use crate::dag::definition::{DAGDefinition, NodeDefinition, EdgeDefinition, NodeType, OutputDestination};

    fn create_simple_dag() -> CompiledDAG {
        let compiler = DAGCompiler::new();
        let mut dag = DAGDefinition::new("test-dag", "Test DAG");
        dag.add_node(NodeDefinition::new("input", NodeType::TextInput));
        dag.add_node(NodeDefinition::new("output", NodeType::TextOutput {
            destination: OutputDestination::WebSocket,
        }));
        dag.add_edge(EdgeDefinition::new("input", "output"));
        dag.with_entry("input");
        dag.add_exit("output");
        compiler.compile(dag).unwrap()
    }

    #[tokio::test]
    async fn test_simple_execution() {
        let dag = create_simple_dag();
        let executor = DAGExecutor::new();
        let mut ctx = DAGContext::new("test-stream");

        let input = DAGData::Text("hello world".to_string());
        let output = executor.execute(&dag, input, &mut ctx).await.unwrap();

        assert!(matches!(output, DAGData::Text(_)));
    }

    #[tokio::test]
    async fn test_execution_with_timeout() {
        let dag = create_simple_dag();
        let executor = DAGExecutor::new().with_timeout(Duration::from_millis(100));
        let mut ctx = DAGContext::new("test-stream");

        let input = DAGData::Text("hello".to_string());
        let result = executor.execute(&dag, input, &mut ctx).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execution_cancelled() {
        let dag = create_simple_dag();
        let executor = DAGExecutor::new();
        let mut ctx = DAGContext::new("test-stream");

        // Cancel before execution
        ctx.cancel_token.cancel();

        let input = DAGData::Text("hello".to_string());
        let result = executor.execute(&dag, input, &mut ctx).await;

        assert!(matches!(result, Err(DAGError::Cancelled)));
    }

    #[tokio::test]
    async fn test_api_key_routing() {
        let compiler = DAGCompiler::new();
        let mut def = DAGDefinition::new("test-dag", "Test DAG");
        def.add_node(NodeDefinition::new("input", NodeType::TextInput));
        def.add_node(NodeDefinition::new("handler_a", NodeType::Passthrough));
        def.add_node(NodeDefinition::new("handler_b", NodeType::Passthrough));
        def.add_node(NodeDefinition::new("output", NodeType::TextOutput {
            destination: OutputDestination::WebSocket,
        }));
        def.add_edge(EdgeDefinition::new("input", "handler_a"));
        def.add_edge(EdgeDefinition::new("input", "handler_b"));
        def.add_edge(EdgeDefinition::new("handler_a", "output"));
        def.add_edge(EdgeDefinition::new("handler_b", "output"));
        def.with_entry("input");
        def.add_exit("output");
        def.api_key_routes.insert("tenant_a".to_string(), "handler_a".to_string());

        let dag = compiler.compile(def).unwrap();
        let executor = DAGExecutor::new();

        // Execute with tenant_a API key
        let mut ctx = DAGContext::with_auth("test-stream", None, Some("tenant_a".to_string()));
        let input = DAGData::Text("test".to_string());
        let result = executor.execute(&dag, input, &mut ctx).await;

        assert!(result.is_ok());
    }

    #[test]
    fn test_executor_builder() {
        let executor = DAGExecutor::new()
            .with_timeout(Duration::from_secs(60))
            .without_parallelism()
            .with_max_branches(5);

        assert_eq!(executor.default_timeout, Duration::from_secs(60));
        assert!(!executor.enable_parallelism);
        assert_eq!(executor.max_concurrent_branches, 5);
    }
}
