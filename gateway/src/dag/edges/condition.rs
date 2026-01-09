//! Edge conditions for conditional routing
//!
//! Edges can have conditions that determine when data flows through them.

use std::sync::Arc;
use rhai::AST;

use crate::dag::context::DAGContext;
use crate::dag::error::DAGResult;
use crate::dag::routing::{CompiledCondition, ConditionEvaluator};

/// Edge condition types
#[derive(Debug, Clone)]
pub enum EdgeCondition {
    /// Always allow data to flow
    Always,
    /// Rhai expression condition
    Expression(String),
    /// Simple switch on a field
    Switch(String, Vec<(String, String)>),
    /// API key pattern match
    ApiKey(Vec<(String, String)>),
}

impl EdgeCondition {
    /// Create an always-true condition
    pub fn always() -> Self {
        Self::Always
    }

    /// Create an expression condition
    pub fn expression(expr: impl Into<String>) -> Self {
        Self::Expression(expr.into())
    }

    /// Create a switch condition
    pub fn switch(field: impl Into<String>, cases: Vec<(String, String)>) -> Self {
        Self::Switch(field.into(), cases)
    }

    /// Check if this is an unconditional edge
    pub fn is_unconditional(&self) -> bool {
        matches!(self, Self::Always)
    }
}

/// Compiled edge with condition
#[derive(Clone)]
pub struct CompiledEdge {
    /// Source node ID
    pub from: String,
    /// Target node ID
    pub to: String,
    /// Compiled condition (if any)
    pub condition: Option<CompiledCondition>,
    /// Priority for ordering when multiple edges match
    pub priority: i32,
    /// Transform script (if any)
    pub transform: Option<Arc<AST>>,
}

impl CompiledEdge {
    /// Create a new compiled edge
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            condition: None,
            priority: 0,
            transform: None,
        }
    }

    /// Create with condition
    pub fn with_condition(mut self, condition: CompiledCondition) -> Self {
        self.condition = Some(condition);
        self
    }

    /// Set priority
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Check if edge condition matches
    pub fn matches(
        &self,
        data: &serde_json::Value,
        ctx: &DAGContext,
        evaluator: &ConditionEvaluator,
    ) -> DAGResult<bool> {
        match &self.condition {
            Some(condition) => evaluator.evaluate(condition, data, ctx),
            None => Ok(true),
        }
    }

    /// Check if this edge is unconditional
    pub fn is_unconditional(&self) -> bool {
        self.condition.is_none()
    }
}

impl std::fmt::Debug for CompiledEdge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledEdge")
            .field("from", &self.from)
            .field("to", &self.to)
            .field("has_condition", &self.condition.is_some())
            .field("priority", &self.priority)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_edge_condition_always() {
        let cond = EdgeCondition::always();
        assert!(cond.is_unconditional());
    }

    #[test]
    fn test_edge_condition_expression() {
        let cond = EdgeCondition::expression("is_final == true");
        assert!(!cond.is_unconditional());
    }

    #[test]
    fn test_compiled_edge() {
        let edge = CompiledEdge::new("input", "stt")
            .with_priority(10);

        assert_eq!(edge.from, "input");
        assert_eq!(edge.to, "stt");
        assert_eq!(edge.priority, 10);
        assert!(edge.is_unconditional());
    }
}
