//! Router nodes for conditional routing and parallel execution
//!
//! These nodes handle flow control in DAG pipelines including:
//! - Split: Broadcast input to multiple branches
//! - Join: Aggregate results from multiple branches
//! - Router: Conditional routing based on expressions

use std::sync::Arc;
use async_trait::async_trait;
use rhai::{Dynamic, Array, Scope};
use tracing::{debug, warn, info};

use super::{DAGNode, DAGData, NodeCapability};
use crate::dag::context::DAGContext;
use crate::dag::definition::{JoinStrategy, RouteDefinition};
use crate::dag::error::{DAGError, DAGResult};
use crate::dag::routing::{CompiledCondition, ConditionEvaluator, create_rhai_engine};

/// Split node for parallel execution
///
/// Broadcasts input to multiple branches for concurrent processing.
/// Each branch receives a clone of the input data.
#[derive(Clone)]
pub struct SplitNode {
    id: String,
    branches: Vec<String>,
}

impl SplitNode {
    /// Create a new split node
    pub fn new(id: impl Into<String>, branches: Vec<String>) -> Self {
        Self {
            id: id.into(),
            branches,
        }
    }

    /// Get the branch node IDs
    pub fn branches(&self) -> &[String] {
        &self.branches
    }

    /// Add a branch
    pub fn add_branch(mut self, branch: impl Into<String>) -> Self {
        self.branches.push(branch.into());
        self
    }
}

impl std::fmt::Debug for SplitNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SplitNode")
            .field("id", &self.id)
            .field("branches", &self.branches)
            .finish()
    }
}

#[async_trait]
impl DAGNode for SplitNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "split"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::AudioInput,
            NodeCapability::TextInput,
            NodeCapability::JsonInput,
            NodeCapability::AudioOutput,
            NodeCapability::TextOutput,
            NodeCapability::JsonOutput,
        ]
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        debug!(
            node_id = %self.id,
            branch_count = %self.branches.len(),
            "Splitting input to branches"
        );

        // Store branch info in context for executor to handle parallel execution
        ctx.metadata.insert(
            "split_branches".to_string(),
            self.branches.join(","),
        );

        // Return input - actual splitting is handled by executor
        Ok(input)
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

/// Join node for aggregating parallel results
///
/// Collects results from multiple branches and aggregates them
/// according to the specified strategy.
#[derive(Clone)]
pub struct JoinNode {
    id: String,
    sources: Vec<String>,
    strategy: JoinStrategy,
    selector: Option<String>,
    merge_script: Option<String>,
}

impl JoinNode {
    /// Create a new join node
    pub fn new(id: impl Into<String>, sources: Vec<String>, strategy: JoinStrategy) -> Self {
        Self {
            id: id.into(),
            sources,
            strategy,
            selector: None,
            merge_script: None,
        }
    }

    /// Create a join that returns the first result
    pub fn first(id: impl Into<String>, sources: Vec<String>) -> Self {
        Self::new(id, sources, JoinStrategy::First)
    }

    /// Create a join that waits for all results
    pub fn all(id: impl Into<String>, sources: Vec<String>) -> Self {
        Self::new(id, sources, JoinStrategy::All)
    }

    /// Set selector expression for Best strategy
    pub fn with_selector(mut self, selector: impl Into<String>) -> Self {
        self.selector = Some(selector.into());
        self
    }

    /// Set merge script for Merge strategy
    pub fn with_merge_script(mut self, script: impl Into<String>) -> Self {
        self.merge_script = Some(script.into());
        self
    }

    /// Get the source node IDs
    pub fn sources(&self) -> &[String] {
        &self.sources
    }

    /// Get the join strategy
    pub fn strategy(&self) -> JoinStrategy {
        self.strategy
    }
}

impl std::fmt::Debug for JoinNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinNode")
            .field("id", &self.id)
            .field("sources", &self.sources)
            .field("strategy", &self.strategy)
            .finish()
    }
}

#[async_trait]
impl DAGNode for JoinNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "join"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::AudioInput,
            NodeCapability::TextInput,
            NodeCapability::JsonInput,
            NodeCapability::AudioOutput,
            NodeCapability::TextOutput,
            NodeCapability::JsonOutput,
        ]
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        debug!(
            node_id = %self.id,
            strategy = ?self.strategy,
            source_count = %self.sources.len(),
            "Joining results"
        );

        // Extract results from Multiple input
        let results = match input {
            DAGData::Multiple(items) => items,
            other => vec![other],
        };

        if results.is_empty() {
            return Err(DAGError::EmptyJoin);
        }

        match self.strategy {
            JoinStrategy::First => {
                // Return first non-empty result
                for result in results {
                    if !matches!(result, DAGData::Empty) {
                        return Ok(result);
                    }
                }
                Ok(DAGData::Empty)
            }
            JoinStrategy::All => {
                // Return all results as Multiple
                Ok(DAGData::Multiple(results))
            }
            JoinStrategy::Best => {
                // Select best result using selector expression
                if let Some(ref selector) = self.selector {
                    self.select_best(results, selector, ctx)
                } else {
                    // Default: return first result
                    results.into_iter().next().ok_or(DAGError::EmptyJoin)
                }
            }
            JoinStrategy::Merge => {
                // Merge results using merge script
                if let Some(ref script) = self.merge_script {
                    self.merge_results(results, script, ctx)
                } else {
                    // Default: return all as array
                    Ok(DAGData::Multiple(results))
                }
            }
        }
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

impl JoinNode {
    /// Select the best result using a Rhai selector expression
    ///
    /// The selector expression receives a `results` array and should return
    /// either an index (integer) or the result object directly.
    ///
    /// Example selectors:
    /// - `results.max_by(|r| r.confidence)` - Select by highest confidence
    /// - `results[0]` - Select first result
    /// - `results.find(|r| r.is_final)` - Select first final result
    fn select_best(
        &self,
        results: Vec<DAGData>,
        selector: &str,
        ctx: &DAGContext,
    ) -> DAGResult<DAGData> {
        if results.is_empty() {
            return Err(DAGError::EmptyJoin);
        }

        // Create Rhai engine with array extensions for selection
        let mut engine = create_rhai_engine();

        // Re-enable looping for array iteration in selectors, but with stricter limits
        // to prevent infinite loops or excessive resource usage
        engine.set_allow_looping(true);
        engine.set_max_operations(1_000); // Lower limit when looping is enabled
        engine.set_max_call_levels(16);   // Limit recursion depth

        // Register max_by_field function for selecting best result by numeric field
        engine.register_fn("max_by_field", |arr: Array, field: &str| -> Dynamic {
            let mut best: Option<Dynamic> = None;
            let mut best_score = f64::MIN;

            for item in arr.iter() {
                if let Some(map) = item.clone().try_cast::<rhai::Map>() {
                    if let Some(val) = map.get(field) {
                        // Try to get numeric value using Rhai's built-in conversion
                        let score: f64 = if val.is::<f64>() {
                            val.clone().cast::<f64>()
                        } else if val.is::<i64>() {
                            val.clone().cast::<i64>() as f64
                        } else if val.is::<i32>() {
                            val.clone().cast::<i32>() as f64
                        } else {
                            continue;
                        };

                        if score > best_score {
                            best_score = score;
                            best = Some(item.clone());
                        }
                    }
                }
            }

            best.unwrap_or(Dynamic::UNIT)
        });

        // Create scope with results array
        let mut scope = Scope::new();

        // Convert results to Rhai Dynamic array
        let rhai_results: Array = results
            .iter()
            .map(|r| json_to_dynamic(&r.to_json()))
            .collect();

        scope.push("results", rhai_results.clone());
        scope.push_constant("stream_id", ctx.stream_id.clone());

        if let Some(api_key) = &ctx.api_key {
            scope.push_constant("api_key", api_key.clone());
        }
        if let Some(api_key_id) = &ctx.api_key_id {
            scope.push_constant("api_key_id", api_key_id.clone());
        }

        info!(
            node_id = %self.id,
            selector = %selector,
            result_count = results.len(),
            "Evaluating selector expression"
        );

        // Compile and evaluate the selector expression
        let result = engine.eval_with_scope::<Dynamic>(&mut scope, selector)
            .map_err(|e| DAGError::ConditionError(format!(
                "Selector expression '{}' failed: {}", selector, e
            )))?;

        // Interpret the result
        if let Some(idx) = result.clone().try_cast::<i64>() {
            // Result is an index
            let idx = idx as usize;
            if idx < results.len() {
                return Ok(results.into_iter().nth(idx).unwrap());
            } else {
                return Err(DAGError::ConditionError(format!(
                    "Selector returned invalid index {} for {} results", idx, results.len()
                )));
            }
        }

        // Result might be a map/object - convert back to DAGData
        if let Some(map) = result.clone().try_cast::<rhai::Map>() {
            return Ok(DAGData::Json(dynamic_to_json(&Dynamic::from(map))));
        }

        // Try to find a matching result by comparing JSON
        let result_json = dynamic_to_json(&result);
        for original in results {
            if original.to_json() == result_json {
                return Ok(original);
            }
        }

        // Return as JSON if no match found
        Ok(DAGData::Json(result_json))
    }

    /// Merge results using a Rhai merge script
    ///
    /// The merge script receives a `results` array and should return
    /// the merged result (can be any type).
    ///
    /// Example merge scripts:
    /// ```rhai
    /// let combined = "";
    /// for r in results { combined += r.text; }
    /// combined
    /// ```
    fn merge_results(
        &self,
        results: Vec<DAGData>,
        script: &str,
        ctx: &DAGContext,
    ) -> DAGResult<DAGData> {
        if results.is_empty() {
            return Err(DAGError::EmptyJoin);
        }

        // Create Rhai engine with extensions
        let mut engine = create_rhai_engine();

        // Re-enable looping for merge scripts, but with stricter limits
        // to prevent infinite loops or excessive resource usage
        engine.set_allow_looping(true);
        engine.set_max_operations(1_000); // Lower limit when looping is enabled
        engine.set_max_call_levels(16);   // Limit recursion depth

        // Create scope with results array
        let mut scope = Scope::new();

        // Convert results to Rhai Dynamic array
        let rhai_results: Array = results
            .iter()
            .map(|r| json_to_dynamic(&r.to_json()))
            .collect();

        scope.push("results", rhai_results);
        scope.push_constant("stream_id", ctx.stream_id.clone());

        if let Some(api_key) = &ctx.api_key {
            scope.push_constant("api_key", api_key.clone());
        }
        if let Some(api_key_id) = &ctx.api_key_id {
            scope.push_constant("api_key_id", api_key_id.clone());
        }

        info!(
            node_id = %self.id,
            script_len = script.len(),
            result_count = results.len(),
            "Evaluating merge script"
        );

        // Compile and evaluate the merge script
        let ast = engine.compile(script)
            .map_err(|e| DAGError::ExpressionCompilationError {
                expression: script.to_string(),
                error: e.to_string(),
            })?;

        let result = engine.eval_ast_with_scope::<Dynamic>(&mut scope, &ast)
            .map_err(|e| DAGError::ConditionError(format!(
                "Merge script failed: {}", e
            )))?;

        // Convert result to DAGData
        if let Some(s) = result.clone().try_cast::<String>() {
            return Ok(DAGData::Text(s));
        }
        if let Some(i) = result.clone().try_cast::<i64>() {
            return Ok(DAGData::Json(serde_json::json!(i)));
        }
        if let Some(f) = result.clone().try_cast::<f64>() {
            return Ok(DAGData::Json(serde_json::json!(f)));
        }
        if let Some(b) = result.clone().try_cast::<bool>() {
            return Ok(DAGData::Json(serde_json::json!(b)));
        }
        if let Some(arr) = result.clone().try_cast::<Array>() {
            let json_arr: Vec<serde_json::Value> = arr.iter()
                .map(|d| dynamic_to_json(d))
                .collect();
            return Ok(DAGData::Json(serde_json::json!(json_arr)));
        }
        if let Some(map) = result.clone().try_cast::<rhai::Map>() {
            return Ok(DAGData::Json(dynamic_to_json(&Dynamic::from(map))));
        }

        // Return as JSON by default
        Ok(DAGData::Json(dynamic_to_json(&result)))
    }
}

/// Convert serde_json::Value to Rhai Dynamic
fn json_to_dynamic(value: &serde_json::Value) -> Dynamic {
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

/// Router node for conditional routing
///
/// Evaluates conditions and routes data to the matching target node.
#[derive(Clone)]
pub struct RouterNode {
    id: String,
    routes: Vec<CompiledRoute>,
}

/// A compiled route with condition
#[derive(Clone)]
pub struct CompiledRoute {
    pub target: String,
    pub condition: Option<CompiledCondition>,
    pub priority: i32,
    pub is_default: bool,
}

impl RouterNode {
    /// Create a new router node
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            routes: Vec::new(),
        }
    }

    /// Create from route definitions
    pub fn from_definitions(
        id: impl Into<String>,
        definitions: Vec<RouteDefinition>,
        evaluator: &ConditionEvaluator,
    ) -> DAGResult<Self> {
        let mut routes = Vec::with_capacity(definitions.len());

        for def in definitions {
            let condition = if let Some(ref expr) = def.condition {
                Some(evaluator.compile_expression(expr)?)
            } else {
                None
            };

            routes.push(CompiledRoute {
                target: def.target,
                condition,
                priority: def.priority,
                is_default: def.default,
            });
        }

        // Sort by priority (highest first)
        routes.sort_by(|a, b| b.priority.cmp(&a.priority));

        Ok(Self {
            id: id.into(),
            routes,
        })
    }

    /// Add a route
    pub fn add_route(mut self, route: CompiledRoute) -> Self {
        self.routes.push(route);
        self.routes.sort_by(|a, b| b.priority.cmp(&a.priority));
        self
    }

    /// Add a conditional route
    pub fn with_condition(
        mut self,
        target: impl Into<String>,
        condition: CompiledCondition,
    ) -> Self {
        self.routes.push(CompiledRoute {
            target: target.into(),
            condition: Some(condition),
            priority: 0,
            is_default: false,
        });
        self
    }

    /// Add a default route
    pub fn with_default(mut self, target: impl Into<String>) -> Self {
        self.routes.push(CompiledRoute {
            target: target.into(),
            condition: None,
            priority: i32::MIN,
            is_default: true,
        });
        self
    }

    /// Get routes
    pub fn routes(&self) -> &[CompiledRoute] {
        &self.routes
    }

    /// Find matching route based on conditions
    ///
    /// Routes are evaluated in priority order (highest priority first).
    /// - Routes with conditions: evaluated against the data and context
    /// - Default route: matches only if no other route matches
    /// - Routes without conditions and not default: never match (configuration error)
    pub fn find_route(
        &self,
        data: &serde_json::Value,
        ctx: &DAGContext,
        evaluator: &ConditionEvaluator,
    ) -> DAGResult<Option<&str>> {
        // First pass: check routes with conditions (non-default)
        for route in &self.routes {
            if route.is_default {
                continue; // Skip default routes in first pass
            }

            if let Some(ref condition) = route.condition {
                if evaluator.evaluate(condition, data, ctx)? {
                    return Ok(Some(&route.target));
                }
            }
            // Routes without conditions and not default are skipped
            // (they should have conditions to be useful)
        }

        // Second pass: check default route as fallback
        for route in &self.routes {
            if route.is_default {
                return Ok(Some(&route.target));
            }
        }

        Ok(None)
    }
}

impl std::fmt::Debug for RouterNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouterNode")
            .field("id", &self.id)
            .field("route_count", &self.routes.len())
            .finish()
    }
}

#[async_trait]
impl DAGNode for RouterNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "router"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::AudioInput,
            NodeCapability::TextInput,
            NodeCapability::JsonInput,
            NodeCapability::AudioOutput,
            NodeCapability::TextOutput,
            NodeCapability::JsonOutput,
        ]
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        debug!(
            node_id = %self.id,
            route_count = %self.routes.len(),
            "Evaluating routes"
        );

        let data_json = input.to_json();
        let evaluator = ConditionEvaluator::with_default_engine();

        // Find matching route
        let target = self.find_route(&data_json, ctx, &evaluator)?;

        if let Some(target_id) = target {
            debug!(
                node_id = %self.id,
                target = %target_id,
                "Route matched"
            );
            ctx.metadata.insert("router_target".to_string(), target_id.to_string());
        } else {
            warn!(
                node_id = %self.id,
                "No route matched"
            );
            return Err(DAGError::NoMatchingRoute(self.id.clone()));
        }

        // Pass through input - actual routing is handled by executor
        Ok(input)
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_node() {
        let node = SplitNode::new("split", vec!["branch_a".into(), "branch_b".into()]);
        assert_eq!(node.id(), "split");
        assert_eq!(node.branches().len(), 2);
    }

    #[test]
    fn test_join_strategies() {
        let node = JoinNode::new("join", vec!["a".into(), "b".into()], JoinStrategy::First);
        assert_eq!(node.strategy(), JoinStrategy::First);

        let node = JoinNode::all("join", vec!["a".into()]);
        assert_eq!(node.strategy(), JoinStrategy::All);
    }

    #[tokio::test]
    async fn test_join_first() {
        let node = JoinNode::first("join", vec!["a".into()]);
        let mut ctx = DAGContext::new("test");

        let input = DAGData::Multiple(vec![
            DAGData::Text("first".into()),
            DAGData::Text("second".into()),
        ]);

        let output = node.execute(input, &mut ctx).await.unwrap();
        if let DAGData::Text(text) = output {
            assert_eq!(text, "first");
        } else {
            panic!("Expected text output");
        }
    }

    #[tokio::test]
    async fn test_join_all() {
        let node = JoinNode::all("join", vec!["a".into(), "b".into()]);
        let mut ctx = DAGContext::new("test");

        let input = DAGData::Multiple(vec![
            DAGData::Text("a".into()),
            DAGData::Text("b".into()),
        ]);

        let output = node.execute(input, &mut ctx).await.unwrap();
        assert!(matches!(output, DAGData::Multiple(_)));
    }

    #[test]
    fn test_router_node() {
        let node = RouterNode::new("router")
            .with_default("default_handler");

        assert_eq!(node.id(), "router");
        assert_eq!(node.routes().len(), 1);
    }

    #[tokio::test]
    async fn test_router_default_route() {
        let node = RouterNode::new("router")
            .with_default("default_handler");

        let mut ctx = DAGContext::new("test");
        let input = DAGData::Text("test".into());

        let output = node.execute(input, &mut ctx).await.unwrap();
        assert_eq!(ctx.metadata.get("router_target"), Some(&"default_handler".to_string()));
    }
}
