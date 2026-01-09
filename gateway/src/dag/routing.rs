//! Expression-based and API key-based routing
//!
//! This module provides the routing logic for DAG edges including:
//! - Rhai expression evaluation for dynamic conditions
//! - Simple switch pattern matching for field-based routing
//! - API key-based routing for tenant isolation

use std::collections::HashMap;
use std::sync::Arc;

use rhai::{Engine, AST, Scope, Dynamic};
use tracing::debug;

use super::context::DAGContext;
use super::definition::SwitchPattern;
use super::error::{DAGError, DAGResult};

/// Compiled condition for edge routing
#[derive(Debug, Clone)]
pub enum CompiledCondition {
    /// Rhai AST for expression evaluation
    Expression {
        ast: Arc<AST>,
        source: String,
    },
    /// Simple field-based switch
    Switch {
        field_segments: Vec<String>,
        cases: HashMap<String, String>,
        default: Option<String>,
    },
    /// API key pattern match
    ApiKey {
        patterns: HashMap<String, String>,
        default: Option<String>,
    },
    /// Always true (unconditional edge)
    Always,
}

/// Condition evaluator for DAG routing
pub struct ConditionEvaluator {
    engine: Arc<Engine>,
}

impl ConditionEvaluator {
    /// Create a new condition evaluator with a configured Rhai engine
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }

    /// Create a condition evaluator with default Rhai engine
    pub fn with_default_engine() -> Self {
        Self::new(Arc::new(create_rhai_engine()))
    }

    /// Compile a Rhai expression condition
    pub fn compile_expression(&self, expression: &str) -> DAGResult<CompiledCondition> {
        let ast = self.engine.compile_expression(expression).map_err(|e| {
            DAGError::ExpressionCompilationError {
                expression: expression.to_string(),
                error: e.to_string(),
            }
        })?;

        Ok(CompiledCondition::Expression {
            ast: Arc::new(ast),
            source: expression.to_string(),
        })
    }

    /// Compile a switch pattern condition
    pub fn compile_switch(&self, pattern: &SwitchPattern) -> DAGResult<CompiledCondition> {
        Ok(CompiledCondition::Switch {
            field_segments: pattern.field.split('.').map(String::from).collect(),
            cases: pattern.cases.clone(),
            default: pattern.default.clone(),
        })
    }

    /// Compile API key routing patterns
    pub fn compile_api_key_routes(
        &self,
        routes: &HashMap<String, String>,
    ) -> DAGResult<CompiledCondition> {
        if routes.is_empty() {
            return Ok(CompiledCondition::Always);
        }

        Ok(CompiledCondition::ApiKey {
            patterns: routes.clone(),
            default: None,
        })
    }

    /// Evaluate a condition against the current context and data
    pub fn evaluate(
        &self,
        condition: &CompiledCondition,
        data: &serde_json::Value,
        ctx: &DAGContext,
    ) -> DAGResult<bool> {
        match condition {
            CompiledCondition::Expression { ast, source } => {
                self.evaluate_expression(ast, source, data, ctx)
            }
            CompiledCondition::Switch { field_segments, cases, default } => {
                self.evaluate_switch(field_segments, cases, default, data)
            }
            CompiledCondition::ApiKey { patterns, default } => {
                self.evaluate_api_key(patterns, default, ctx)
            }
            CompiledCondition::Always => Ok(true),
        }
    }

    /// Get the target node from a switch or API key condition
    pub fn get_target(
        &self,
        condition: &CompiledCondition,
        data: &serde_json::Value,
        ctx: &DAGContext,
    ) -> DAGResult<Option<String>> {
        match condition {
            CompiledCondition::Switch { field_segments, cases, default } => {
                let value = extract_field(data, field_segments)?;
                let value_str = json_value_to_string(&value);

                if let Some(target) = cases.get(&value_str) {
                    return Ok(Some(target.clone()));
                }
                Ok(default.clone())
            }
            CompiledCondition::ApiKey { patterns, default } => {
                if let Some(api_key_id) = &ctx.api_key_id {
                    // Try exact match first
                    if let Some(target) = patterns.get(api_key_id) {
                        return Ok(Some(target.clone()));
                    }
                    // Try prefix match
                    for (pattern, target) in patterns {
                        if api_key_id.starts_with(pattern) {
                            return Ok(Some(target.clone()));
                        }
                    }
                }
                Ok(default.clone())
            }
            _ => Ok(None), // Expression and Always don't provide targets
        }
    }

    /// Evaluate a Rhai expression
    fn evaluate_expression(
        &self,
        ast: &AST,
        source: &str,
        data: &serde_json::Value,
        ctx: &DAGContext,
    ) -> DAGResult<bool> {
        let mut scope = Scope::new();

        // Add context variables
        scope.push_constant("stream_id", ctx.stream_id.clone());
        if let Some(api_key) = &ctx.api_key {
            scope.push_constant("api_key", api_key.clone());
        }
        if let Some(api_key_id) = &ctx.api_key_id {
            scope.push_constant("api_key_id", api_key_id.clone());
        }

        // Add metadata
        for (key, value) in &ctx.metadata {
            scope.push_constant(key.clone(), value.clone());
        }

        // Add data fields to scope
        populate_scope_from_json(&mut scope, "data", data);

        // Common fields for STT results
        if let Some(transcript) = data.get("transcript").and_then(|v| v.as_str()) {
            scope.push_constant("transcript", transcript.to_string());
        }
        if let Some(is_final) = data.get("is_final").and_then(|v| v.as_bool()) {
            scope.push_constant("is_final", is_final);
        }
        if let Some(is_speech_final) = data.get("is_speech_final").and_then(|v| v.as_bool()) {
            scope.push_constant("is_speech_final", is_speech_final);
        }
        if let Some(confidence) = data.get("confidence").and_then(|v| v.as_f64()) {
            scope.push_constant("confidence", confidence);
        }

        // Evaluate the expression
        let result: Dynamic = self.engine.eval_ast_with_scope(&mut scope, ast).map_err(|e| {
            debug!(source = %source, error = %e, "Expression evaluation failed");
            DAGError::ConditionError(format!("Expression '{}' failed: {}", source, e))
        })?;

        // Convert result to bool
        result.as_bool().map_err(|_| {
            DAGError::ConditionError(format!(
                "Expression '{}' did not return a boolean, got: {:?}",
                source, result
            ))
        })
    }

    /// Evaluate a switch pattern
    fn evaluate_switch(
        &self,
        field_segments: &[String],
        cases: &HashMap<String, String>,
        default: &Option<String>,
        data: &serde_json::Value,
    ) -> DAGResult<bool> {
        let value = extract_field(data, field_segments)?;
        let value_str = json_value_to_string(&value);

        Ok(cases.contains_key(&value_str) || default.is_some())
    }

    /// Evaluate API key routing
    fn evaluate_api_key(
        &self,
        patterns: &HashMap<String, String>,
        default: &Option<String>,
        ctx: &DAGContext,
    ) -> DAGResult<bool> {
        if let Some(api_key_id) = &ctx.api_key_id {
            // Exact match
            if patterns.contains_key(api_key_id) {
                return Ok(true);
            }
            // Prefix match
            for pattern in patterns.keys() {
                if api_key_id.starts_with(pattern) {
                    return Ok(true);
                }
            }
        }
        Ok(default.is_some())
    }
}

impl Default for ConditionEvaluator {
    fn default() -> Self {
        Self::with_default_engine()
    }
}

/// Create a configured Rhai engine for DAG condition evaluation
pub fn create_rhai_engine() -> Engine {
    let mut engine = Engine::new();

    // Sandboxing: limit resource usage
    engine.set_max_expr_depths(64, 64);
    engine.set_max_operations(10_000);
    engine.set_max_modules(0); // No external modules
    engine.set_max_string_size(10_000);
    engine.set_max_array_size(1_000);
    engine.set_max_map_size(1_000);

    // Disable certain features for security
    engine.set_allow_looping(false); // No loops
    // Allow undefined variables at compile time - they will be bound at runtime
    // via populate_scope_from_json and explicit scope.push_constant calls
    engine.set_strict_variables(false);

    // Register custom functions
    engine.register_fn("len", |s: &str| s.len() as i64);
    engine.register_fn("is_empty", |s: &str| s.is_empty());
    engine.register_fn("contains", |s: &str, sub: &str| s.contains(sub));
    engine.register_fn("starts_with", |s: &str, prefix: &str| s.starts_with(prefix));
    engine.register_fn("ends_with", |s: &str, suffix: &str| s.ends_with(suffix));
    engine.register_fn("to_lower", |s: &str| s.to_lowercase());
    engine.register_fn("to_upper", |s: &str| s.to_uppercase());
    engine.register_fn("trim", |s: &str| s.trim().to_string());

    // Numeric functions
    engine.register_fn("abs", |x: i64| x.abs());
    engine.register_fn("abs", |x: f64| x.abs());
    engine.register_fn("min", |a: i64, b: i64| a.min(b));
    engine.register_fn("max", |a: i64, b: i64| a.max(b));
    engine.register_fn("min", |a: f64, b: f64| a.min(b));
    engine.register_fn("max", |a: f64, b: f64| a.max(b));

    engine
}

/// Extract a field from JSON data using a path
fn extract_field(data: &serde_json::Value, segments: &[String]) -> DAGResult<serde_json::Value> {
    let mut current = data;

    for segment in segments {
        current = current.get(segment).ok_or_else(|| DAGError::FieldExtractionError {
            field: segments.join("."),
            error: format!("Field '{}' not found", segment),
        })?;
    }

    Ok(current.clone())
}

/// Convert JSON value to string for switch matching
fn json_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => "null".to_string(),
        _ => value.to_string(),
    }
}

/// Populate Rhai scope from JSON object
fn populate_scope_from_json(scope: &mut Scope, prefix: &str, value: &serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                let full_key = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}_{}", prefix, key)
                };

                match val {
                    serde_json::Value::String(s) => {
                        scope.push_constant(full_key, s.clone());
                    }
                    serde_json::Value::Bool(b) => {
                        scope.push_constant(full_key, *b);
                    }
                    serde_json::Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            scope.push_constant(full_key, i);
                        } else if let Some(f) = n.as_f64() {
                            scope.push_constant(full_key, f);
                        }
                    }
                    serde_json::Value::Null => {
                        scope.push_constant(full_key, ());
                    }
                    _ => {} // Skip arrays and nested objects for now
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_expression_compilation() {
        let evaluator = ConditionEvaluator::with_default_engine();
        let condition = evaluator.compile_expression("is_final == true").unwrap();
        assert!(matches!(condition, CompiledCondition::Expression { .. }));
    }

    #[test]
    fn test_expression_evaluation() {
        let evaluator = ConditionEvaluator::with_default_engine();
        let condition = evaluator.compile_expression("is_final == true").unwrap();
        let ctx = DAGContext::new("stream-123");

        let data = json!({ "is_final": true, "transcript": "hello" });
        let result = evaluator.evaluate(&condition, &data, &ctx).unwrap();
        assert!(result);

        let data = json!({ "is_final": false, "transcript": "hello" });
        let result = evaluator.evaluate(&condition, &data, &ctx).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_switch_pattern() {
        let evaluator = ConditionEvaluator::with_default_engine();
        let pattern = SwitchPattern::new("language")
            .add_case("en-US", "english_handler")
            .add_case("es-ES", "spanish_handler")
            .with_default("default_handler");

        let condition = evaluator.compile_switch(&pattern).unwrap();
        let ctx = DAGContext::new("stream-123");

        let data = json!({ "language": "en-US" });
        assert!(evaluator.evaluate(&condition, &data, &ctx).unwrap());

        let target = evaluator.get_target(&condition, &data, &ctx).unwrap();
        assert_eq!(target, Some("english_handler".to_string()));

        // Test default
        let data = json!({ "language": "fr-FR" });
        let target = evaluator.get_target(&condition, &data, &ctx).unwrap();
        assert_eq!(target, Some("default_handler".to_string()));
    }

    #[test]
    fn test_api_key_routing() {
        let evaluator = ConditionEvaluator::with_default_engine();
        let mut routes = HashMap::new();
        routes.insert("tenant_a".to_string(), "handler_a".to_string());
        routes.insert("tenant_b".to_string(), "handler_b".to_string());

        let condition = evaluator.compile_api_key_routes(&routes).unwrap();

        let ctx = DAGContext::with_auth("stream-123", None, Some("tenant_a".to_string()));
        let data = json!({});

        assert!(evaluator.evaluate(&condition, &data, &ctx).unwrap());

        let target = evaluator.get_target(&condition, &data, &ctx).unwrap();
        assert_eq!(target, Some("handler_a".to_string()));
    }

    #[test]
    fn test_complex_expression() {
        let evaluator = ConditionEvaluator::with_default_engine();
        let condition = evaluator
            .compile_expression("is_final && confidence > 0.8")
            .unwrap();

        let ctx = DAGContext::new("stream-123");

        let data = json!({ "is_final": true, "confidence": 0.9 });
        assert!(evaluator.evaluate(&condition, &data, &ctx).unwrap());

        let data = json!({ "is_final": true, "confidence": 0.7 });
        assert!(!evaluator.evaluate(&condition, &data, &ctx).unwrap());
    }

    #[test]
    fn test_string_functions() {
        let evaluator = ConditionEvaluator::with_default_engine();
        let condition = evaluator
            .compile_expression("transcript.contains(\"hello\")")
            .unwrap();

        let ctx = DAGContext::new("stream-123");

        let data = json!({ "transcript": "hello world" });
        assert!(evaluator.evaluate(&condition, &data, &ctx).unwrap());

        let data = json!({ "transcript": "goodbye world" });
        assert!(!evaluator.evaluate(&condition, &data, &ctx).unwrap());
    }

    #[test]
    fn test_field_extraction() {
        let data = json!({
            "stt_result": {
                "is_final": true,
                "language": "en-US"
            }
        });

        let segments: Vec<String> = vec!["stt_result".into(), "language".into()];
        let value = extract_field(&data, &segments).unwrap();
        assert_eq!(value, json!("en-US"));
    }

    #[test]
    fn test_always_condition() {
        let evaluator = ConditionEvaluator::with_default_engine();
        let condition = CompiledCondition::Always;
        let ctx = DAGContext::new("stream-123");
        let data = json!({});

        assert!(evaluator.evaluate(&condition, &data, &ctx).unwrap());
    }
}
