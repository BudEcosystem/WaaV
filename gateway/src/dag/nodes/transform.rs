//! Transform node for data transformation
//!
//! This node uses Rhai scripts to transform data between nodes.

use std::sync::Arc;
use async_trait::async_trait;
use rhai::{Engine, AST, Scope, Dynamic};
use tracing::debug;

use super::{DAGNode, DAGData, NodeCapability};
use crate::dag::context::DAGContext;
use crate::dag::error::{DAGError, DAGResult};
use crate::dag::routing::create_rhai_engine;

/// Transform node for data transformation using Rhai scripts
///
/// Executes a Rhai script to transform input data before passing it
/// to the next node. Useful for format conversion, field extraction,
/// and data manipulation.
#[derive(Clone)]
pub struct TransformNode {
    id: String,
    script_source: String,
    compiled_ast: Option<Arc<AST>>,
    engine: Arc<Engine>,
}

impl TransformNode {
    /// Create a new transform node
    pub fn new(id: impl Into<String>, script: impl Into<String>) -> Self {
        let engine = Arc::new(create_rhai_engine());
        let script_source = script.into();

        Self {
            id: id.into(),
            script_source,
            compiled_ast: None,
            engine,
        }
    }

    /// Compile the transform script
    pub fn compile(&mut self) -> DAGResult<()> {
        let ast = self.engine.compile(&self.script_source).map_err(|e| {
            DAGError::ExpressionCompilationError {
                expression: self.script_source.clone(),
                error: e.to_string(),
            }
        })?;

        self.compiled_ast = Some(Arc::new(ast));
        Ok(())
    }

    /// Create a compiled transform node
    pub fn compiled(id: impl Into<String>, script: impl Into<String>) -> DAGResult<Self> {
        let mut node = Self::new(id, script);
        node.compile()?;
        Ok(node)
    }

    /// Get the script source
    pub fn script(&self) -> &str {
        &self.script_source
    }

    /// Check if the script is compiled
    pub fn is_compiled(&self) -> bool {
        self.compiled_ast.is_some()
    }
}

impl std::fmt::Debug for TransformNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformNode")
            .field("id", &self.id)
            .field("script_length", &self.script_source.len())
            .field("compiled", &self.is_compiled())
            .finish()
    }
}

#[async_trait]
impl DAGNode for TransformNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "transform"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::TextInput,
            NodeCapability::JsonInput,
            NodeCapability::TextOutput,
            NodeCapability::JsonOutput,
        ]
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        debug!(
            node_id = %self.id,
            input_type = %input.type_name(),
            "Executing transform"
        );

        // Compile if not already compiled
        let ast = match &self.compiled_ast {
            Some(ast) => ast.clone(),
            None => {
                // Compile on demand
                Arc::new(self.engine.compile(&self.script_source).map_err(|e| {
                    DAGError::ExpressionCompilationError {
                        expression: self.script_source.clone(),
                        error: e.to_string(),
                    }
                })?)
            }
        };

        // Prepare scope with input data
        let mut scope = Scope::new();

        // Add context variables
        scope.push_constant("stream_id", ctx.stream_id.clone());
        if let Some(api_key_id) = &ctx.api_key_id {
            scope.push_constant("api_key_id", api_key_id.clone());
        }

        // Add metadata
        for (key, value) in &ctx.metadata {
            scope.push_constant(key.clone(), value.clone());
        }

        // Add input data based on type
        match &input {
            DAGData::Text(text) => {
                scope.push_constant("input", text.clone());
                scope.push_constant("input_type", "text");
            }
            DAGData::STTResult(stt) => {
                scope.push_constant("transcript", stt.transcript.clone());
                scope.push_constant("is_final", stt.is_final);
                scope.push_constant("is_speech_final", stt.is_speech_final);
                scope.push_constant("confidence", stt.confidence);
                scope.push_constant("input_type", "stt_result");
            }
            DAGData::Json(json) => {
                // Add JSON fields to scope
                if let Some(obj) = json.as_object() {
                    for (key, value) in obj {
                        match value {
                            serde_json::Value::String(s) => {
                                scope.push_constant(key.clone(), s.clone());
                            }
                            serde_json::Value::Bool(b) => {
                                scope.push_constant(key.clone(), *b);
                            }
                            serde_json::Value::Number(n) => {
                                if let Some(i) = n.as_i64() {
                                    scope.push_constant(key.clone(), i);
                                } else if let Some(f) = n.as_f64() {
                                    scope.push_constant(key.clone(), f);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                scope.push_constant("input_type", "json");
            }
            DAGData::Empty => {
                scope.push_constant("input_type", "empty");
                return Ok(DAGData::Empty);
            }
            other => {
                scope.push_constant("input_type", other.type_name());
            }
        }

        // Execute the transform script
        let result: Dynamic = self.engine.eval_ast_with_scope(&mut scope, &ast).map_err(|e| {
            DAGError::node_error(&self.id, format!("Transform script failed: {}", e))
        })?;

        // Convert result to DAGData
        let output = dynamic_to_dag_data(result);

        debug!(
            node_id = %self.id,
            output_type = %output.type_name(),
            "Transform completed"
        );

        Ok(output)
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

/// Convert Rhai Dynamic to DAGData
fn dynamic_to_dag_data(value: Dynamic) -> DAGData {
    if value.is::<String>() {
        DAGData::Text(value.cast::<String>())
    } else if value.is::<bool>() {
        let b = value.cast::<bool>();
        DAGData::Json(serde_json::json!(b))
    } else if value.is::<i64>() {
        let n = value.cast::<i64>();
        DAGData::Json(serde_json::json!(n))
    } else if value.is::<f64>() {
        let f = value.cast::<f64>();
        DAGData::Json(serde_json::json!(f))
    } else if value.is::<rhai::Array>() {
        let arr = value.cast::<rhai::Array>();
        let json_arr: Vec<serde_json::Value> = arr
            .into_iter()
            .map(|v| dynamic_to_json(&v))
            .collect();
        DAGData::Json(serde_json::Value::Array(json_arr))
    } else if value.is::<rhai::Map>() {
        let map = value.cast::<rhai::Map>();
        let json_obj: serde_json::Map<String, serde_json::Value> = map
            .into_iter()
            .map(|(k, v)| (k.to_string(), dynamic_to_json(&v)))
            .collect();
        DAGData::Json(serde_json::Value::Object(json_obj))
    } else if value.is_unit() {
        DAGData::Empty
    } else {
        // Default: convert to string
        DAGData::Text(value.to_string())
    }
}

/// Convert Rhai Dynamic to serde_json::Value
fn dynamic_to_json(value: &Dynamic) -> serde_json::Value {
    if value.is::<String>() {
        serde_json::json!(value.clone().cast::<String>())
    } else if value.is::<bool>() {
        serde_json::json!(value.clone().cast::<bool>())
    } else if value.is::<i64>() {
        serde_json::json!(value.clone().cast::<i64>())
    } else if value.is::<f64>() {
        serde_json::json!(value.clone().cast::<f64>())
    } else if value.is::<rhai::Array>() {
        let arr = value.clone().cast::<rhai::Array>();
        let json_arr: Vec<serde_json::Value> = arr.iter().map(dynamic_to_json).collect();
        serde_json::Value::Array(json_arr)
    } else if value.is::<rhai::Map>() {
        let map = value.clone().cast::<rhai::Map>();
        let json_obj: serde_json::Map<String, serde_json::Value> = map
            .iter()
            .map(|(k, v)| (k.to_string(), dynamic_to_json(v)))
            .collect();
        serde_json::Value::Object(json_obj)
    } else if value.is_unit() {
        serde_json::Value::Null
    } else {
        serde_json::json!(value.to_string())
    }
}

/// Passthrough node (no-op)
///
/// Simply passes input through unchanged. Useful for graph organization
/// and as a placeholder during development.
#[derive(Debug, Clone)]
pub struct PassthroughNode {
    id: String,
}

impl PassthroughNode {
    /// Create a new passthrough node
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

#[async_trait]
impl DAGNode for PassthroughNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "passthrough"
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

    async fn execute(&self, input: DAGData, _ctx: &mut DAGContext) -> DAGResult<DAGData> {
        debug!(node_id = %self.id, "Passthrough");
        Ok(input)
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_transform_text() {
        let node = TransformNode::compiled(
            "transform",
            r#"input.to_upper()"#
        ).unwrap();

        let mut ctx = DAGContext::new("test");
        let input = DAGData::Text("hello".to_string());

        let output = node.execute(input, &mut ctx).await.unwrap();
        if let DAGData::Text(text) = output {
            assert_eq!(text, "HELLO");
        } else {
            panic!("Expected text output");
        }
    }

    #[tokio::test]
    async fn test_transform_stt_result() {
        let node = TransformNode::compiled(
            "transform",
            r#"if is_final { transcript } else { "" }"#
        ).unwrap();

        let mut ctx = DAGContext::new("test");
        let input = DAGData::STTResult(super::super::STTResultData {
            transcript: "hello world".to_string(),
            is_final: true,
            ..Default::default()
        });

        let output = node.execute(input, &mut ctx).await.unwrap();
        if let DAGData::Text(text) = output {
            assert_eq!(text, "hello world");
        } else {
            panic!("Expected text output");
        }
    }

    #[tokio::test]
    async fn test_transform_returns_object() {
        let node = TransformNode::compiled(
            "transform",
            r#"#{ text: input, processed: true }"#
        ).unwrap();

        let mut ctx = DAGContext::new("test");
        let input = DAGData::Text("test".to_string());

        let output = node.execute(input, &mut ctx).await.unwrap();
        assert!(matches!(output, DAGData::Json(_)));
    }

    #[tokio::test]
    async fn test_passthrough() {
        let node = PassthroughNode::new("pass");
        let mut ctx = DAGContext::new("test");

        let input = DAGData::Text("unchanged".to_string());
        let output = node.execute(input.clone(), &mut ctx).await.unwrap();

        if let (DAGData::Text(inp), DAGData::Text(out)) = (input, output) {
            assert_eq!(inp, out);
        } else {
            panic!("Expected matching text");
        }
    }

    #[test]
    fn test_transform_compilation() {
        let node = TransformNode::compiled("t", "1 + 1");
        assert!(node.is_ok());
        assert!(node.unwrap().is_compiled());

        let node = TransformNode::compiled("t", "invalid syntax {{{");
        assert!(node.is_err());
    }
}
