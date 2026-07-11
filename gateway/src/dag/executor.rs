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
use tokio::sync::{Semaphore, mpsc};
use tokio::time::timeout;
use tracing::{debug, info, warn};

use super::compiler::CompiledDAG;
use super::context::{DAGContext, DagOutput};
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
        Self::record_outcome(dag, &result, duration);

        result
    }

    /// Execute the DAG starting at a specific node id, feeding `input` into it.
    ///
    /// This is the entry point for the streaming data-plane (W-O1): STT runs in the
    /// VoiceManager and, on a finalized turn, the StreamDriver injects a
    /// `DAGData::STTResult` at the **post-STT node** (the node directly downstream of
    /// the STT provider node) and runs the rest of the graph once. The upstream
    /// `audio_input`/`stt_provider` nodes are intentionally NOT re-run (the audio was
    /// already transcribed), so we start partway through the topological order.
    ///
    /// Output nodes deliver their results through `ctx.output_tx` (see `DagOutput`).
    pub async fn execute_from(
        &self,
        dag: &CompiledDAG,
        start_node_id: &str,
        input: DAGData,
        ctx: &mut DAGContext,
    ) -> DAGResult<DAGData> {
        let start_time = Instant::now();

        let start_node = *dag
            .node_index
            .get(start_node_id)
            .ok_or(DAGError::InvalidStartNode)?;

        debug!(
            dag_id = %dag.definition.id,
            start_node = %dag.graph[start_node].id,
            input_type = %input.type_name(),
            "Starting DAG execution from explicit node"
        );

        let result = self.execute_from_node(dag, start_node, input, ctx).await;

        let duration = start_time.elapsed();
        Self::record_outcome(dag, &result, duration);

        result
    }

    /// Collect the nodes reachable from `start` IF they form a streamable TREE — every reachable
    /// node (besides `start`) has at most one incoming edge from within the set, i.e. there is NO
    /// join / fan-in. Linear chains AND fan-out (split / conditional router) qualify; a join (>1
    /// incoming) does not, because a join must synchronize all its inputs before it can emit and so
    /// is a batch boundary. Returns `None` on a join or a cycle (the caller falls back to batch).
    fn streamable_tree(&self, dag: &CompiledDAG, start: NodeIndex) -> Option<Vec<NodeIndex>> {
        let mut order = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![start];
        while let Some(n) = stack.pop() {
            if !seen.insert(n) {
                return None; // re-reached ⇒ join or cycle
            }
            if n != start && dag.incoming_edges(n).len() > 1 {
                return None; // fan-in / join — needs synchronization, not streaming
            }
            order.push(n);
            for (next, _e) in dag.outgoing_edges(n) {
                stack.push(next);
            }
        }
        Some(order)
    }

    /// Run the subgraph from `start_node_id` in STREAMING mode (the streaming data-plane).
    ///
    /// EVERY reachable node runs concurrently, wired by bounded channels, so a downstream node
    /// (TTS) starts producing as soon as an upstream node (an LLM) emits its first chunk — slashing
    /// time-to-first-output for real-time voice. Each node runs [`DAGNode::execute_streaming`]
    /// (non-streaming nodes use the default one-in/one-out adapter, so `stt(batch) → llm(stream) →
    /// tts(stream)` works seamlessly). Handles arbitrary FAN-OUT: a node with several outgoing
    /// edges broadcasts each chunk to every downstream (a split), or — when an edge carries a
    /// condition — only to the matching downstream (a router), evaluated per chunk. Terminal nodes
    /// (no outgoing edge) deliver to `ctx.output_tx` (the client sink) as chunks arrive, so a
    /// `… → [tts→audio, text_out]` fan-out streams both audio and text incrementally.
    ///
    /// A JOIN (any reachable node with >1 incoming edge) is a synchronization point that cannot
    /// stream, so such graphs transparently fall back to the batch [`execute_from`](Self::execute_from).
    pub async fn execute_streaming_from(
        &self,
        dag: &CompiledDAG,
        start_node_id: &str,
        input: DAGData,
        ctx: &mut DAGContext,
    ) -> DAGResult<()> {
        let start = *dag
            .node_index
            .get(start_node_id)
            .ok_or(DAGError::InvalidStartNode)?;
        let nodes = match self.streamable_tree(dag, start) {
            Some(n) => n,
            None => {
                self.execute_from(dag, start_node_id, input, ctx).await?;
                return Ok(());
            }
        };

        const CHAN_BUF: usize = 64;

        // Execution-scoped cancellation: a CHILD of the shared session token, so (a) barge-in /
        // session cancellation still propagates into every task spawned here, and (b) on a node
        // failure we can cancel ALL of this run's tasks — including forwarders blocked on a full
        // bounded channel — without poisoning the caller's token for subsequent turns.
        let cancel = ctx.cancel_token.child_token();

        // One input channel per node. Edge A→B means A's forwarder sends to B's input sender.
        let mut in_tx: HashMap<NodeIndex, mpsc::Sender<DAGData>> = HashMap::new();
        let mut in_rx: HashMap<NodeIndex, mpsc::Receiver<DAGData>> = HashMap::new();
        for &n in &nodes {
            let (tx, rx) = mpsc::channel::<DAGData>(CHAN_BUF);
            in_tx.insert(n, tx);
            in_rx.insert(n, rx);
        }

        let mut node_tasks: tokio::task::JoinSet<Result<(), (String, DAGError)>> =
            tokio::task::JoinSet::new();
        let mut fwd_handles = Vec::with_capacity(nodes.len());
        for &n in &nodes {
            let node = Arc::clone(&dag.graph[n].node);
            let node_id = dag.graph[n].id.clone();
            let rx = in_rx.remove(&n).ok_or_else(|| {
                DAGError::InternalError(format!(
                    "streaming executor missing input receiver for node '{node_id}'"
                ))
            })?;
            let (out_tx, mut out_rx) = mpsc::channel::<DAGData>(CHAN_BUF);

            // The node task: stream this node's outputs onto `out_tx`. Own context per node; the
            // client sink is cleared (only the terminal forwarder below delivers) and the cancel
            // token is the execution-scoped child (barge-in still propagates through the parent).
            let mut node_ctx = ctx.clone_for_branch();
            node_ctx.output_tx = None;
            node_ctx.cancel_token = cancel.clone();
            node_tasks.spawn(async move {
                node.execute_streaming(rx, &mut node_ctx, out_tx)
                    .await
                    .map_err(|e| (node_id, e))
            });

            // The forwarder task: route this node's stream to its downstream inputs (per-chunk
            // condition eval for routers; broadcast for splits) or to the client sink if terminal.
            let downstream: Vec<(mpsc::Sender<DAGData>, super::edges::CompiledEdge)> = dag
                .outgoing_edges(n)
                .into_iter()
                .filter_map(|(tgt, edge)| in_tx.get(&tgt).cloned().map(|tx| (tx, edge.clone())))
                .collect();
            let evaluator = Arc::clone(&dag.evaluator);
            let fwd_ctx = ctx.clone_for_branch();
            let sink = ctx.output_tx.clone();
            let fwd_cancel = cancel.clone();
            // Every await in the forwarder selects on `fwd_cancel` so a forwarder blocked on a
            // full channel (downstream stalled or sink reader gone) terminates on cancellation
            // instead of leaking.
            fwd_handles.push(tokio::spawn(async move {
                'fwd: loop {
                    let chunk = tokio::select! {
                        biased;
                        _ = fwd_cancel.cancelled() => break 'fwd,
                        chunk = out_rx.recv() => match chunk {
                            Some(c) => c,
                            None => break 'fwd,
                        },
                    };
                    if downstream.is_empty() {
                        // Terminal node → deliver to the client as produced.
                        if let Some(s) = &sink {
                            let out = match &chunk {
                                DAGData::TTSAudio(a) => Some(DagOutput::Audio(a.clone())),
                                DAGData::Text(t) => Some(DagOutput::Text(t.clone())),
                                _ => None,
                            };
                            if let Some(o) = out {
                                tokio::select! {
                                    biased;
                                    _ = fwd_cancel.cancelled() => break 'fwd,
                                    res = s.send(o) => { let _ = res; }
                                }
                            }
                        }
                        continue;
                    }
                    let chunk_json = chunk.to_json();
                    for (tx, edge) in &downstream {
                        let pass = match &edge.condition {
                            Some(c) => evaluator
                                .evaluate(c, &chunk_json, &fwd_ctx)
                                .unwrap_or(false),
                            None => true,
                        };
                        if pass {
                            tokio::select! {
                                biased;
                                _ = fwd_cancel.cancelled() => break 'fwd,
                                res = tx.send(chunk.clone()) => { let _ = res; }
                            }
                        }
                    }
                }
            }));
        }

        // Feed the input to the root and drop ALL executor-held senders. Each non-root node's input
        // now has senders only from its single parent forwarder, so the close cascades root→leaves
        // as each forwarder finishes.
        if let Some(s) = in_tx.get(&start) {
            let _ = s.send(input).await;
        }
        drop(in_tx);

        // Await node tasks in COMPLETION order. On the FIRST failure (error return or panic),
        // cancel the execution-scoped token so every remaining task — upstream nodes still
        // producing and forwarders blocked on bounded channels — terminates promptly instead of
        // leaking, then keep draining so nothing outlives this call (MASTER_PLAN §5:
        // streaming-forwarder leak).
        let mut first_err: Option<DAGError> = None;
        while let Some(res) = node_tasks.join_next().await {
            let err = match res {
                Ok(Ok(())) => None,
                Ok(Err((node_id, e))) => Some(DAGError::NodeExecutionError {
                    node_id,
                    error: e.to_string(),
                }),
                Err(join) => Some(DAGError::NodeExecutionError {
                    node_id: "<join>".into(),
                    error: format!("streaming node task failed: {join}"),
                }),
            };
            if let Some(e) = err {
                cancel.cancel();
                first_err.get_or_insert(e);
            }
        }
        // Forwarders end when their node's output closes — or via `cancel` on the failure path.
        for h in fwd_handles {
            let _ = h.await;
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Record success/failure/cancellation metrics + tracing for a finished run.
    fn record_outcome(dag: &CompiledDAG, result: &DAGResult<DAGData>, duration: Duration) {
        match result {
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
        let start_pos = dag
            .topo_order
            .iter()
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

        // Track which nodes have already executed. Split branch interiors are
        // executed by `execute_split` and recorded here so the outer
        // topological sweep skips them — each node executes exactly once
        // (W-O3 bug #1).
        let mut executed: HashSet<NodeIndex> = HashSet::new();

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

            // Skip nodes already executed as part of a split branch interior.
            if executed.contains(&node_idx) {
                continue;
            }

            let compiled_node = &dag.graph[node_idx];
            let node_type = compiled_node.node.node_type();

            // ── Split-aware handling (W-O3 bug #1/#2) ───────────────────────
            // When we reach a split node, execute the split itself once, then
            // run each branch's interior subtree exactly once (in parallel),
            // recording branch outputs and marking interior nodes executed so
            // the outer sweep does not re-run them.
            if node_type == "split" {
                let split_input = self
                    .gather_inputs_with_transforms(dag, node_idx, &node_outputs, ctx)
                    .await?;
                if matches!(split_input, DAGData::Empty) && node_idx != start {
                    continue;
                }
                // Execute the split node itself (passthrough; sets metadata).
                let split_output = self
                    .execute_node(dag, node_idx, split_input.clone(), ctx)
                    .await?;
                node_outputs.insert(node_idx, split_output);

                // Run the precomputed branch interiors exactly once.
                self.run_split_branches(dag, node_idx, &split_input, ctx, &mut node_outputs)
                    .await?;

                // Mark every interior node executed so the outer sweep skips it.
                if let Some(plans) = dag.split_plans.get(&node_idx) {
                    for plan in plans {
                        for &interior_idx in &plan.interior {
                            executed.insert(interior_idx);
                        }
                    }
                }

                executed.insert(node_idx);
                continue;
            }

            // Gather input. Join nodes read their declared `sources` from
            // `node_outputs` by id so parallel branch outputs are never lost
            // (W-O3 bug #2); all other nodes gather from graph predecessors with
            // edge transforms applied.
            //
            // The start node is special: its input was seeded into `node_outputs`
            // by the caller (e.g. the StreamDriver injecting an STTResult at the
            // post-STT node, W-O1). Its graph predecessors were NOT executed, so we
            // must use the seeded input directly rather than gathering Empty from
            // them.
            let node_input = if node_idx == start {
                node_outputs
                    .get(&node_idx)
                    .cloned()
                    .unwrap_or(DAGData::Empty)
            } else if node_type == "join" {
                self.gather_join_inputs(dag, node_idx, &node_outputs)
            } else {
                self.gather_inputs_with_transforms(dag, node_idx, &node_outputs, ctx)
                    .await?
            };

            // Skip if no input (conditional edge didn't match)
            if matches!(node_input, DAGData::Empty) && node_idx != start {
                continue;
            }

            // Execute the node
            let output = self.execute_node(dag, node_idx, node_input, ctx).await?;
            executed.insert(node_idx);

            // Handle router node output - prune unreachable downstream paths
            if compiled_node.node.node_type() == "router"
                && let Some(router_target) = ctx.metadata.get("router_target").cloned()
            {
                // Find the target node index
                if let Some(target_idx) = dag.get_node_index(&router_target) {
                    // Prune all outgoing paths except the target
                    self.prune_non_target_paths(dag, node_idx, target_idx, &mut reachable_nodes);
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
            let has_other_path = incoming
                .iter()
                .any(|(source, _)| *source != router_idx && reachable_nodes.contains(source));

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
            let has_other_path = incoming
                .iter()
                .any(|(source, _)| *source != node_idx && reachable_nodes.contains(source));

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

    /// Gather inputs for a Join node from its declared `sources` (by node id).
    ///
    /// Unlike the generic predecessor gather, this reads each declared source's
    /// output from `node_outputs` directly, so the aggregate of all parallel
    /// branch outputs reaches the Join regardless of edge-condition/order quirks
    /// (W-O3 bug #2). Sources that produced no output are skipped. Falls back to
    /// the graph-predecessor gather if the node is not actually a Join.
    fn gather_join_inputs(
        &self,
        dag: &CompiledDAG,
        node_idx: NodeIndex,
        node_outputs: &HashMap<NodeIndex, DAGData>,
    ) -> DAGData {
        use crate::dag::definition::NodeType;

        let compiled = &dag.graph[node_idx];
        let sources: Vec<String> = match &compiled.definition.node_type {
            NodeType::Join { sources, .. } => sources.clone(),
            _ => Vec::new(),
        };

        // If sources are declared, read them by id; otherwise fall back to the
        // join node's incoming graph predecessors.
        let source_indices: Vec<NodeIndex> = if sources.is_empty() {
            dag.incoming_edges(node_idx)
                .into_iter()
                .map(|(src, _)| src)
                .collect()
        } else {
            sources
                .iter()
                .filter_map(|id| dag.get_node_index(id))
                .collect()
        };

        let mut inputs: Vec<DAGData> = Vec::new();
        for src_idx in source_indices {
            if let Some(data) = node_outputs.get(&src_idx)
                && !matches!(data, DAGData::Empty)
            {
                inputs.push(data.clone());
            }
        }

        match inputs.len() {
            0 => DAGData::Empty,
            1 => inputs.remove(0),
            _ => DAGData::Multiple(inputs),
        }
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
            return Ok(node_outputs
                .get(&node_idx)
                .cloned()
                .unwrap_or(DAGData::Empty));
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
        // Note: create_rhai_engine() already sets:
        // - set_allow_looping(false) - prevents DoS via infinite loops
        // - set_max_operations(10_000)
        // - set_max_call_levels(32)
        // Edge transforms should use Rhai's built-in array methods (map, filter, reduce)
        // instead of explicit loops for iteration.
        let engine = create_rhai_engine();

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
        let result = engine
            .eval_ast_with_scope::<Dynamic>(&mut scope, transform_ast)
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
        self.gather_inputs_with_transforms(dag, node_idx, node_outputs, ctx)
            .await
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

        // All node executions (including Split, which is now a passthrough that
        // records branch metadata) are wrapped with a timeout. Split branch
        // fan-out is handled by `run_split_branches` in the topological sweep so
        // each node executes exactly once.
        //
        // Honor the PER-NODE `timeout_ms` (a slow node — e.g. a reasoning LLM — can be given more
        // time than fast peers); fall back to the executor's `default_timeout` when unset.
        let node_timeout = compiled_node
            .definition
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(self.default_timeout);
        let _ = node_type; // retained for tracing above
        let result = match timeout(node_timeout, node.execute(input, ctx)).await {
            Ok(result) => result,
            Err(_) => {
                warn!(
                    node_id = %node_id,
                    timeout_ms = %node_timeout.as_millis(),
                    "Node execution timed out"
                );
                Err(DAGError::ExecutionTimeout(node_timeout.as_millis() as u64))
            }
        };

        let duration = start.elapsed();

        // Record metrics
        dag.metrics
            .record_node_execution(node_id, duration, result.is_ok());

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
                ctx.set_node_result_arc(node_id.clone(), Arc::new(output.to_json()));
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

    /// Execute the precomputed branch interiors of a split node exactly once.
    ///
    /// Each branch's interior subtree (from the compiler's `split_plans`) is
    /// executed in its own cloned branch context. Branches run in parallel,
    /// bounded by `max_concurrent_branches` (W-O3 bug #5). The output of every
    /// interior node is merged back into the shared `node_outputs` map (keyed by
    /// node index) and each branch's node results are merged back into the main
    /// context, so the downstream Join node — which executes once in the main
    /// sweep — can read its declared `sources` from `node_outputs` by id
    /// (W-O3 bug #2: no parallel-branch data loss).
    async fn run_split_branches(
        &self,
        dag: &CompiledDAG,
        split_idx: NodeIndex,
        input: &DAGData,
        ctx: &mut DAGContext,
        node_outputs: &mut HashMap<NodeIndex, DAGData>,
    ) -> DAGResult<()> {
        use futures::future::join_all;

        let plans = match dag.split_plans.get(&split_idx) {
            Some(p) if !p.is_empty() => p,
            _ => {
                warn!(
                    node_id = %dag.graph[split_idx].id,
                    "Split node has no branch plans"
                );
                return Ok(());
            }
        };

        // Semaphore bounds the number of branches executing concurrently.
        let permits = self.max_concurrent_branches.max(1);
        let semaphore = Arc::new(Semaphore::new(permits));

        // Build one future per branch. Each future executes that branch's
        // interior nodes in topological order in an isolated branch context and
        // returns the per-node outputs plus the branch's node-result map.
        let mut branch_futures = Vec::with_capacity(plans.len());
        for plan in plans {
            let branch_ctx = ctx.clone_for_branch();
            let input_clone = input.clone();
            let sem = semaphore.clone();
            branch_futures.push(async move {
                let _permit = sem
                    .acquire_owned()
                    .await
                    .map_err(|e| DAGError::InternalError(e.to_string()))?;
                let mut branch_ctx = branch_ctx;
                let outputs = self
                    .execute_branch_interior(dag, plan, input_clone, &mut branch_ctx)
                    .await
                    .map_err(|e| DAGError::SplitBranchError {
                        branch_id: plan.branch_id.clone(),
                        error: e.to_string(),
                    })?;
                Ok::<_, DAGError>((outputs, branch_ctx))
            });
        }

        // Execute branches (concurrently when parallelism is enabled).
        let branch_results: Vec<DAGResult<(HashMap<NodeIndex, DAGData>, DAGContext)>> =
            if self.enable_parallelism {
                join_all(branch_futures).await
            } else {
                let mut out = Vec::with_capacity(branch_futures.len());
                for fut in branch_futures {
                    out.push(fut.await);
                }
                out
            };

        // Merge every branch's outputs and node results back into the shared
        // state. The first error aborts the whole split.
        for result in branch_results {
            let (outputs, branch_ctx) = result?;
            for (idx, data) in outputs {
                node_outputs.insert(idx, data);
            }
            // Merge branch context node results back into the main context so
            // condition/transform evaluation downstream can see them.
            for key in branch_ctx.result_keys().cloned().collect::<Vec<_>>() {
                if let Some(arc) = branch_ctx.get_node_result(&key) {
                    ctx.set_node_result_arc(key, arc.clone());
                }
            }
        }

        Ok(())
    }

    /// Execute a single branch's interior nodes in topological order, once.
    ///
    /// Returns the output of every interior node keyed by node index. The branch
    /// context is mutated in place so its node results can be merged back by the
    /// caller. Interior nodes never include the reconvergence/join node, which
    /// runs once in the main sweep.
    async fn execute_branch_interior(
        &self,
        dag: &CompiledDAG,
        plan: &super::compiler::SplitBranchPlan,
        split_output: DAGData,
        branch_ctx: &mut DAGContext,
    ) -> DAGResult<HashMap<NodeIndex, DAGData>> {
        let interior: HashSet<NodeIndex> = plan.interior.iter().copied().collect();
        let mut outputs: HashMap<NodeIndex, DAGData> = HashMap::new();

        // The branch entry receives the split's output directly.
        outputs.insert(plan.entry, split_output);

        for &node_idx in &plan.interior {
            if !branch_ctx.should_continue() {
                return Err(if branch_ctx.is_cancelled() {
                    DAGError::Cancelled
                } else {
                    DAGError::ExecutionTimeout(self.default_timeout.as_millis() as u64)
                });
            }

            // Gather inputs from interior predecessors only. For the branch
            // entry, predecessors outside the interior (the split) are ignored
            // here because its input was injected above.
            let node_input = if node_idx == plan.entry {
                outputs.get(&plan.entry).cloned().unwrap_or(DAGData::Empty)
            } else {
                self.gather_inputs_with_transforms(dag, node_idx, &outputs, branch_ctx)
                    .await?
            };

            if matches!(node_input, DAGData::Empty) && node_idx != plan.entry {
                continue;
            }

            let output = self
                .execute_node(dag, node_idx, node_input, branch_ctx)
                .await?;
            outputs.insert(node_idx, output);
        }

        // Only return outputs for interior nodes (defensive).
        outputs.retain(|idx, _| interior.contains(idx));
        Ok(outputs)
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
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = DAGResult<Vec<DAGData>>> + Send + 'a>>
    {
        Box::pin(async move {
            use futures::future::join_all;

            if !self.enable_parallelism || branches.len() <= 1 {
                // Sequential execution
                let mut results = Vec::with_capacity(branches.len());
                for branch_id in branches {
                    let branch_idx = dag
                        .get_node_index(branch_id)
                        .ok_or_else(|| DAGError::UnknownNode(branch_id.clone()))?;

                    let mut branch_ctx = ctx.clone_for_branch();
                    // Use Box::pin for recursive call
                    let result = Box::pin(self.execute_from_node(
                        dag,
                        branch_idx,
                        input.clone(),
                        &mut branch_ctx,
                    ))
                    .await?;
                    results.push(result);
                }
                return Ok(results);
            }

            // Parallel execution using join_all (avoids spawning and lifetime issues)
            // First, resolve all branch indices upfront
            let mut branch_indices = Vec::with_capacity(branches.len());
            for branch_id in branches {
                let branch_idx = dag
                    .get_node_index(branch_id)
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
                        Box::pin(self.execute_from_node(
                            dag,
                            branch_idx,
                            input_clone,
                            &mut branch_ctx,
                        ))
                        .await
                    })
                        as std::pin::Pin<
                            Box<dyn std::future::Future<Output = DAGResult<DAGData>> + Send>,
                        >
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
                        let branch_id = branch_indices
                            .get(i)
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
        let json_arr: Vec<serde_json::Value> = arr.iter().map(dynamic_to_json).collect();
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
            v.clone()
                .try_cast::<i64>()
                .map(|i| (0..=255).contains(&i))
                .unwrap_or(false)
        }) {
            let bytes: Vec<u8> = arr
                .iter()
                .filter_map(|v| v.clone().try_cast::<i64>().map(|i| i as u8))
                .collect();
            return Ok(DAGData::Binary(Bytes::from(bytes)));
        }

        // Otherwise treat as JSON array
        let json_arr: Vec<serde_json::Value> = arr.iter().map(dynamic_to_json).collect();
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
    use crate::dag::definition::{
        DAGDefinition, EdgeDefinition, NodeDefinition, NodeType, OutputDestination,
    };

    fn create_simple_dag() -> CompiledDAG {
        let compiler = DAGCompiler::new();
        let mut dag = DAGDefinition::new("test-dag", "Test DAG");
        dag.add_node(NodeDefinition::new("input", NodeType::TextInput));
        dag.add_node(NodeDefinition::new(
            "output",
            NodeType::TextOutput {
                destination: OutputDestination::WebSocket,
            },
        ));
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
        def.add_node(NodeDefinition::new(
            "output",
            NodeType::TextOutput {
                destination: OutputDestination::WebSocket,
            },
        ));
        def.add_edge(EdgeDefinition::new("input", "handler_a"));
        def.add_edge(EdgeDefinition::new("input", "handler_b"));
        def.add_edge(EdgeDefinition::new("handler_a", "output"));
        def.add_edge(EdgeDefinition::new("handler_b", "output"));
        def.with_entry("input");
        def.add_exit("output");
        def.api_key_routes
            .insert("tenant_a".to_string(), "handler_a".to_string());

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

    // ─────────────────────────────────────────────────────────────────────────
    // Split / Join correctness tests (W-O3 bugs #1 and #2)
    // ─────────────────────────────────────────────────────────────────────────

    use crate::dag::context::DAGContext;
    use crate::dag::definition::JoinStrategy;
    use crate::dag::nodes::{DAGData, DAGNode, NodeCapability};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A test node that counts how many times it executes and tags the output
    /// with its own id so Join data-flow can be asserted.
    struct CountingNode {
        id: String,
        count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl DAGNode for CountingNode {
        fn id(&self) -> &str {
            &self.id
        }
        fn node_type(&self) -> &str {
            "counting"
        }
        fn capabilities(&self) -> Vec<NodeCapability> {
            vec![
                NodeCapability::TextInput,
                NodeCapability::JsonInput,
                NodeCapability::TextOutput,
                NodeCapability::JsonOutput,
            ]
        }
        async fn execute(&self, _input: DAGData, _ctx: &mut DAGContext) -> DAGResult<DAGData> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(DAGData::Text(self.id.clone()))
        }
        fn clone_boxed(&self) -> Arc<dyn DAGNode> {
            Arc::new(CountingNode {
                id: self.id.clone(),
                count: self.count.clone(),
            })
        }
    }

    /// Build a split → 2 branches → join → output DAG where each branch node is
    /// a `CountingNode`. Returns the compiled DAG and the two per-branch counters.
    fn build_split_join_dag(
        join_strategy: JoinStrategy,
    ) -> (CompiledDAG, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        use crate::dag::definition::NodeType;

        let compiler = DAGCompiler::new();
        let mut def = DAGDefinition::new("split-join", "Split Join DAG");
        def.add_node(NodeDefinition::new("input", NodeType::TextInput));
        def.add_node(NodeDefinition::new(
            "split",
            NodeType::Split {
                branches: vec!["branch_a".into(), "branch_b".into()],
            },
        ));
        // Branch nodes are passthroughs in the definition; we swap them for
        // CountingNodes after compilation.
        def.add_node(NodeDefinition::new("branch_a", NodeType::Passthrough));
        def.add_node(NodeDefinition::new("branch_b", NodeType::Passthrough));
        def.add_node(NodeDefinition::new(
            "join",
            NodeType::Join {
                sources: vec!["branch_a".into(), "branch_b".into()],
                strategy: join_strategy,
                selector: None,
                merge_script: None,
            },
        ));
        // Passthrough exit so the join's aggregate (a `Multiple`) reaches the
        // exit intact for assertion (a TextOutput node would reject Multiple).
        def.add_node(NodeDefinition::new("output", NodeType::Passthrough));

        def.add_edge(EdgeDefinition::new("input", "split"));
        def.add_edge(EdgeDefinition::new("split", "branch_a"));
        def.add_edge(EdgeDefinition::new("split", "branch_b"));
        def.add_edge(EdgeDefinition::new("branch_a", "join"));
        def.add_edge(EdgeDefinition::new("branch_b", "join"));
        def.add_edge(EdgeDefinition::new("join", "output"));
        def.with_entry("input");
        def.add_exit("output");

        let mut dag = compiler.compile(def).unwrap();

        let count_a = Arc::new(AtomicUsize::new(0));
        let count_b = Arc::new(AtomicUsize::new(0));

        let idx_a = dag.get_node_index("branch_a").unwrap();
        let idx_b = dag.get_node_index("branch_b").unwrap();
        dag.graph[idx_a].node = Arc::new(CountingNode {
            id: "branch_a".to_string(),
            count: count_a.clone(),
        });
        dag.graph[idx_b].node = Arc::new(CountingNode {
            id: "branch_b".to_string(),
            count: count_b.clone(),
        });

        (dag, count_a, count_b)
    }

    /// Bug #1: each branch node must execute EXACTLY once (no double execution
    /// from the recursive split AND the outer topo sweep).
    #[tokio::test]
    async fn test_split_branch_executes_exactly_once() {
        let (dag, count_a, count_b) = build_split_join_dag(JoinStrategy::All);
        let executor = DAGExecutor::new();
        let mut ctx = DAGContext::new("test-stream");

        let input = DAGData::Text("hello".to_string());
        let _ = executor.execute(&dag, input, &mut ctx).await.unwrap();

        assert_eq!(
            count_a.load(Ordering::SeqCst),
            1,
            "branch_a should execute exactly once, got {}",
            count_a.load(Ordering::SeqCst)
        );
        assert_eq!(
            count_b.load(Ordering::SeqCst),
            1,
            "branch_b should execute exactly once, got {}",
            count_b.load(Ordering::SeqCst)
        );
    }

    /// Bug #2: parallel split execution must produce the same result as
    /// sequential execution, and the Join must see BOTH branch outputs (no
    /// data loss).
    #[tokio::test]
    async fn test_parallel_result_equals_sequential() {
        fn join_outputs(data: &DAGData) -> Vec<String> {
            // Collect the leaf Text values so order-independent comparison is possible.
            let mut out = Vec::new();
            fn walk(d: &DAGData, acc: &mut Vec<String>) {
                match d {
                    DAGData::Text(s) => acc.push(s.clone()),
                    DAGData::Multiple(items) => {
                        for i in items {
                            walk(i, acc);
                        }
                    }
                    other => acc.push(other.to_json().to_string()),
                }
            }
            walk(data, &mut out);
            out.sort();
            out
        }

        // Parallel
        let (dag_par, _a1, _b1) = build_split_join_dag(JoinStrategy::All);
        let exec_par = DAGExecutor::new(); // parallelism on
        let mut ctx_par = DAGContext::new("stream-par");
        let res_par = exec_par
            .execute(&dag_par, DAGData::Text("x".into()), &mut ctx_par)
            .await
            .unwrap();

        // Sequential
        let (dag_seq, _a2, _b2) = build_split_join_dag(JoinStrategy::All);
        let exec_seq = DAGExecutor::new().without_parallelism();
        let mut ctx_seq = DAGContext::new("stream-seq");
        let res_seq = exec_seq
            .execute(&dag_seq, DAGData::Text("x".into()), &mut ctx_seq)
            .await
            .unwrap();

        let par = join_outputs(&res_par);
        let seq = join_outputs(&res_seq);

        // The join must carry BOTH branch outputs through to the exit.
        assert!(
            par.contains(&"branch_a".to_string()) && par.contains(&"branch_b".to_string()),
            "parallel result must contain both branch outputs, got {:?}",
            par
        );
        assert_eq!(par, seq, "parallel result must equal sequential result");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Streaming task cleanup (MASTER_PLAN §5: streaming-forwarder leak)
    // ─────────────────────────────────────────────────────────────────────────

    /// RAII guard tracking live `execute_streaming` tasks: +1 on construction,
    /// −1 on drop — so the counter is exact even on early return / error paths.
    struct TaskGuard(Arc<AtomicUsize>);
    impl TaskGuard {
        fn enter(counter: &Arc<AtomicUsize>) -> Self {
            counter.fetch_add(1, Ordering::SeqCst);
            Self(counter.clone())
        }
    }
    impl Drop for TaskGuard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Streams ticks indefinitely after consuming its seed input — like a live
    /// STT/LLM provider mid-generation. Only cancellation or downstream closure
    /// stops it; its INPUT channel closing does not.
    struct InfiniteSourceNode {
        id: String,
        live: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl DAGNode for InfiniteSourceNode {
        fn id(&self) -> &str {
            &self.id
        }
        fn node_type(&self) -> &str {
            "infinite_source"
        }
        fn capabilities(&self) -> Vec<NodeCapability> {
            vec![NodeCapability::TextInput, NodeCapability::TextOutput]
        }
        async fn execute(&self, _input: DAGData, _ctx: &mut DAGContext) -> DAGResult<DAGData> {
            Ok(DAGData::Text("tick".into()))
        }
        async fn execute_streaming(
            &self,
            mut inputs: mpsc::Receiver<DAGData>,
            ctx: &mut DAGContext,
            outputs: mpsc::Sender<DAGData>,
        ) -> DAGResult<()> {
            let _live = TaskGuard::enter(&self.live);
            let _ = inputs.recv().await; // consume the seed
            loop {
                tokio::select! {
                    _ = ctx.cancel_token.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(2)) => {
                        if outputs.send(DAGData::Text("tick".into())).await.is_err() {
                            break; // downstream gone
                        }
                    }
                }
            }
            Ok(())
        }
        fn clone_boxed(&self) -> Arc<dyn DAGNode> {
            Arc::new(Self {
                id: self.id.clone(),
                live: self.live.clone(),
            })
        }
    }

    /// Fails on the first chunk it receives — the "middle node errors" case.
    struct FailingMidNode {
        id: String,
        live: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl DAGNode for FailingMidNode {
        fn id(&self) -> &str {
            &self.id
        }
        fn node_type(&self) -> &str {
            "failing_mid"
        }
        fn capabilities(&self) -> Vec<NodeCapability> {
            vec![NodeCapability::TextInput, NodeCapability::TextOutput]
        }
        async fn execute(&self, _input: DAGData, _ctx: &mut DAGContext) -> DAGResult<DAGData> {
            Err(DAGError::NodeExecutionError {
                node_id: self.id.clone(),
                error: "boom".into(),
            })
        }
        async fn execute_streaming(
            &self,
            mut inputs: mpsc::Receiver<DAGData>,
            _ctx: &mut DAGContext,
            _outputs: mpsc::Sender<DAGData>,
        ) -> DAGResult<()> {
            let _live = TaskGuard::enter(&self.live);
            let _ = inputs.recv().await; // wait for the first chunk, then fail
            Err(DAGError::NodeExecutionError {
                node_id: self.id.clone(),
                error: "boom".into(),
            })
        }
        fn clone_boxed(&self) -> Arc<dyn DAGNode> {
            Arc::new(Self {
                id: self.id.clone(),
                live: self.live.clone(),
            })
        }
    }

    /// Passthrough that tracks task liveness.
    struct GuardedPassthroughNode {
        id: String,
        live: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl DAGNode for GuardedPassthroughNode {
        fn id(&self) -> &str {
            &self.id
        }
        fn node_type(&self) -> &str {
            "guarded_passthrough"
        }
        fn capabilities(&self) -> Vec<NodeCapability> {
            vec![NodeCapability::TextInput, NodeCapability::TextOutput]
        }
        async fn execute(&self, input: DAGData, _ctx: &mut DAGContext) -> DAGResult<DAGData> {
            Ok(input)
        }
        async fn execute_streaming(
            &self,
            mut inputs: mpsc::Receiver<DAGData>,
            _ctx: &mut DAGContext,
            outputs: mpsc::Sender<DAGData>,
        ) -> DAGResult<()> {
            let _live = TaskGuard::enter(&self.live);
            while let Some(item) = inputs.recv().await {
                if outputs.send(item).await.is_err() {
                    break; // downstream gone
                }
            }
            Ok(())
        }
        fn clone_boxed(&self) -> Arc<dyn DAGNode> {
            Arc::new(Self {
                id: self.id.clone(),
                live: self.live.clone(),
            })
        }
    }

    /// Build a linear src → mid → sink streaming DAG with the given node impls.
    fn build_streaming_chain(
        src: Arc<dyn DAGNode>,
        mid: Arc<dyn DAGNode>,
        sink: Arc<dyn DAGNode>,
    ) -> CompiledDAG {
        let compiler = DAGCompiler::new();
        let mut def = DAGDefinition::new("stream-chain", "Streaming chain DAG");
        def.add_node(NodeDefinition::new("src", NodeType::Passthrough));
        def.add_node(NodeDefinition::new("mid", NodeType::Passthrough));
        def.add_node(NodeDefinition::new("sink", NodeType::Passthrough));
        def.add_edge(EdgeDefinition::new("src", "mid"));
        def.add_edge(EdgeDefinition::new("mid", "sink"));
        def.with_entry("src");
        def.add_exit("sink");
        let mut dag = compiler.compile(def).unwrap();

        let idx_src = dag.get_node_index("src").unwrap();
        let idx_mid = dag.get_node_index("mid").unwrap();
        let idx_sink = dag.get_node_index("sink").unwrap();
        dag.graph[idx_src].node = src;
        dag.graph[idx_mid].node = mid;
        dag.graph[idx_sink].node = sink;
        dag
    }

    /// MASTER_PLAN §5 (streaming-forwarder leak): when the MIDDLE node of a
    /// 3-node streaming pipeline errors, the executor must surface that error
    /// AND tear down every spawned task — including the infinite upstream
    /// producer and its forwarder. Pre-fix the executor blocked forever on the
    /// source's forwarder (the source never stops on its own), so the 500 ms
    /// deadline IS the leak assertion; the guard counter then proves every
    /// node task actually exited.
    #[tokio::test]
    async fn test_streaming_mid_node_error_terminates_all_tasks() {
        let live = Arc::new(AtomicUsize::new(0));
        let dag = build_streaming_chain(
            Arc::new(InfiniteSourceNode {
                id: "src".into(),
                live: live.clone(),
            }),
            Arc::new(FailingMidNode {
                id: "mid".into(),
                live: live.clone(),
            }),
            Arc::new(GuardedPassthroughNode {
                id: "sink".into(),
                live: live.clone(),
            }),
        );

        let executor = DAGExecutor::new();
        let mut ctx = DAGContext::new("leak-test");

        let result = tokio::time::timeout(
            Duration::from_millis(500),
            executor.execute_streaming_from(&dag, "src", DAGData::Text("go".into()), &mut ctx),
        )
        .await
        .expect("streaming execution must terminate within 500ms after a node error");

        match result {
            Err(DAGError::NodeExecutionError { node_id, .. }) => {
                assert_eq!(
                    node_id, "mid",
                    "error must come from the failing middle node"
                );
            }
            other => panic!("expected mid-node execution error, got {:?}", other),
        }

        // Every node task must have exited (its guard dropped). The executor
        // joins all tasks before returning, so this is already 0; poll briefly
        // anyway so the assertion is about "within 500ms", not instantaneity.
        let deadline = Instant::now() + Duration::from_millis(500);
        while live.load(Ordering::SeqCst) != 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            live.load(Ordering::SeqCst),
            0,
            "all streaming node tasks must terminate after a node error"
        );

        // The failure must cancel only the execution-scoped CHILD token, never
        // the caller's session token (or barge-in state would be poisoned for
        // subsequent turns).
        assert!(
            !ctx.cancel_token.is_cancelled(),
            "session cancel token must not be poisoned by a node failure"
        );
    }

    /// Happy path regression for the cleanup rework: a 3-node passthrough chain
    /// still streams its input through to the client sink and all tasks exit.
    #[tokio::test]
    async fn test_streaming_happy_path_delivers_and_cleans_up() {
        let live = Arc::new(AtomicUsize::new(0));
        let dag = build_streaming_chain(
            Arc::new(GuardedPassthroughNode {
                id: "src".into(),
                live: live.clone(),
            }),
            Arc::new(GuardedPassthroughNode {
                id: "mid".into(),
                live: live.clone(),
            }),
            Arc::new(GuardedPassthroughNode {
                id: "sink".into(),
                live: live.clone(),
            }),
        );

        let executor = DAGExecutor::new();
        let (out_tx, mut out_rx) = mpsc::channel::<DagOutput>(16);
        let mut ctx = DAGContext::new("happy-test");
        ctx.output_tx = Some(out_tx);

        let result = tokio::time::timeout(
            Duration::from_millis(500),
            executor.execute_streaming_from(&dag, "src", DAGData::Text("go".into()), &mut ctx),
        )
        .await
        .expect("streaming execution must terminate");
        assert!(result.is_ok(), "happy path must succeed, got {:?}", result);

        // The terminal forwarder delivered the chunk to the client sink before
        // the executor returned (all tasks are joined first).
        match out_rx.try_recv() {
            Ok(DagOutput::Text(t)) => assert_eq!(t, "go"),
            other => panic!("expected DagOutput::Text(\"go\"), got {:?}", other),
        }
        assert_eq!(live.load(Ordering::SeqCst), 0, "all tasks must have exited");
        assert!(!ctx.cancel_token.is_cancelled());
    }
}
