//! LLM endpoint node for OpenAI-compatible API integration.
//!
//! This DAG node is a thin adapter over the feature-flag-free
//! [`crate::core::llm::LlmClient`]. All of the SSE/streaming/function-call and
//! conversation-history logic lives in `core::llm` so it can be shared with the
//! built-in conversation orchestrator (which must work without `dag-routing`).
//!
//! The node:
//! - deserializes the same JSON config as before (`LlmEndpointConfig`),
//! - extracts the prompt text from the inbound `DAGData`,
//! - delegates the completion to `LlmClient` (keyed by `ctx.stream_id`, so
//!   per-session history is preserved exactly as before),
//! - maps the result back into `DAGData::Json` with the same shape downstream
//!   nodes already expect.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::{DAGData, DAGNode, NodeCapability};
use crate::core::llm::{LlmClient, LlmClientConfig, LlmError, TokenCallback};
use crate::dag::context::DAGContext;
use crate::dag::error::{DAGError, DAGResult};

// Sentence chunking for the streaming path lives in the SHARED aggregator
// (`core::text::SentenceAggregator`) — lookahead + decimal/abbreviation
// disambiguation + multilingual terminators + the 160-char latency cap — so
// the DAG path and the conversation pump can never diverge again
// (PIPECAT_FIX_PLAN C-G1+2).
use crate::core::text::SentenceAggregator;

// Re-export the OpenAI-compatible wire types from the shared core so the DAG's
// public surface (`nodes::llm::{ChatMessage, ResponseFormat, ToolDefinition, ...}`)
// is unchanged for existing callers. These are an API passthrough; not all are
// referenced inside this adapter module.
#[allow(unused_imports)]
pub use crate::core::llm::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ChatMessage, FunctionCall,
    FunctionDefinition, MessageRole, ResponseFormat, ToolCall, ToolDefinition, Usage,
};

fn default_timeout_ms() -> u64 {
    60000
}

fn default_max_history() -> usize {
    20
}

/// LLM endpoint configuration (DAG-node-facing).
///
/// Kept as a distinct type (rather than aliasing `LlmClientConfig`) so the
/// node's JSON schema and `Default` are stable; it converts into
/// `LlmClientConfig` for the shared client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmEndpointConfig {
    /// Custom base URL (e.g., "https://api.openai.com/v1", "http://localhost:11434/v1").
    pub base_url: String,
    /// Model identifier.
    pub model: String,
    /// API key (can use ${ENV_VAR} syntax).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Organization ID (OpenAI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    /// System prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Temperature (0.0 - 2.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Top P (0.0 - 1.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Maximum tokens to generate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Enable streaming responses.
    #[serde(default)]
    pub streaming: bool,
    /// Tool definitions for function calling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    /// Tool choice strategy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    /// Response format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    /// Stop sequences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    /// Request timeout in milliseconds.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Maximum conversation history to maintain.
    #[serde(default = "default_max_history")]
    pub max_history: usize,
    /// Extra headers.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Agent/Assistant ID (for OpenAI Assistants API).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant_id: Option<String>,
    /// Thread ID (for OpenAI Assistants API).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Extra provider-specific parameters.
    #[serde(default)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Default for LlmEndpointConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-4o".to_string(),
            api_key: None,
            organization: None,
            system_prompt: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            streaming: false,
            tools: None,
            tool_choice: None,
            response_format: None,
            stop: None,
            timeout_ms: default_timeout_ms(),
            max_history: default_max_history(),
            headers: HashMap::new(),
            assistant_id: None,
            thread_id: None,
            extra: HashMap::new(),
        }
    }
}

impl From<LlmEndpointConfig> for LlmClientConfig {
    fn from(c: LlmEndpointConfig) -> Self {
        LlmClientConfig {
            base_url: c.base_url,
            model: c.model,
            api_key: c.api_key,
            organization: c.organization,
            system_prompt: c.system_prompt,
            temperature: c.temperature,
            top_p: c.top_p,
            max_tokens: c.max_tokens,
            streaming: c.streaming,
            tools: c.tools,
            tool_choice: c.tool_choice,
            response_format: c.response_format,
            stop: c.stop,
            timeout_ms: c.timeout_ms,
            max_history: c.max_history,
            headers: c.headers,
            assistant_id: c.assistant_id,
            thread_id: c.thread_id,
            extra: c.extra,
        }
    }
}

/// Map a core LLM error to a DAG error.
fn map_llm_error(e: LlmError) -> DAGError {
    match e {
        LlmError::Cancelled => DAGError::Cancelled,
        LlmError::Endpoint {
            provider,
            model,
            error,
        } => DAGError::LlmEndpointError {
            provider,
            model,
            error,
        },
    }
}

/// LLM endpoint node for OpenAI-compatible APIs.
///
/// Supports any OpenAI-compatible API (OpenAI, Azure, Ollama, vLLM, LiteLLM,
/// etc.), streaming and non-streaming responses, function/tool calling,
/// per-stream conversation history, and cancellation — all via the shared
/// `LlmClient`.
#[derive(Clone)]
pub struct LlmEndpointNode {
    id: String,
    streaming: bool,
    client: LlmClient,
}

impl std::fmt::Debug for LlmEndpointNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmEndpointNode")
            .field("id", &self.id)
            .field("client", &self.client)
            .finish()
    }
}

impl LlmEndpointNode {
    /// Create a new LLM endpoint node.
    ///
    /// Note: this constructor does NOT validate `base_url` for SSRF attacks.
    /// Use `try_new()` for client-supplied configs (the compiler does).
    pub fn new(id: impl Into<String>, config: LlmEndpointConfig) -> Self {
        let streaming = config.streaming;
        let client = LlmClient::new(config.into());
        Self {
            id: id.into(),
            streaming,
            client,
        }
    }

    /// Create a new LLM endpoint node with `base_url` SSRF validation.
    ///
    /// Rejects base URLs that point at loopback, link-local, RFC1918, or cloud
    /// metadata endpoints — including DNS names that resolve to those ranges
    /// (resolve-then-validate). The DAG compiler MUST use this for
    /// client-supplied LLM configs (W-O3 bug #3).
    pub fn try_new(id: impl Into<String>, config: LlmEndpointConfig) -> DAGResult<Self> {
        super::endpoint::validate_url_for_ssrf(&config.base_url)?;
        Ok(Self::new(id, config))
    }

    /// Create from JSON config.
    pub fn from_json(id: impl Into<String>, config: serde_json::Value) -> Result<Self, String> {
        let config: LlmEndpointConfig =
            serde_json::from_value(config).map_err(|e| format!("Invalid LLM config: {}", e))?;
        Ok(Self::new(id, config))
    }

    /// The chat-completions endpoint URL (for tests / introspection).
    pub fn chat_completions_url(&self) -> String {
        self.client.chat_completions_url()
    }

    /// Add a tool result to the conversation for a specific stream.
    pub async fn add_tool_result(&self, stream_id: &str, tool_call_id: &str, result: &str) {
        self.client
            .add_tool_result(stream_id, tool_call_id, result)
            .await;
    }

    /// Clear conversation history for a specific stream.
    pub async fn clear_history(&self, stream_id: &str) {
        self.client.clear_history(stream_id).await;
    }

    /// Remove conversation history for a specific stream (cleanup on disconnect).
    pub async fn remove_history(&self, stream_id: &str) {
        self.client.remove_history(stream_id).await;
    }

    /// Number of streams with active history (for monitoring).
    pub async fn active_stream_count(&self) -> usize {
        self.client.active_session_count().await
    }
}

#[async_trait]
impl DAGNode for LlmEndpointNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "llm_endpoint"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::TextInput,
            NodeCapability::JsonInput,
            NodeCapability::TextOutput,
            NodeCapability::JsonOutput,
            NodeCapability::Cancellable,
            if self.streaming {
                NodeCapability::Streaming
            } else {
                NodeCapability::TextOutput
            },
        ]
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        // Extract text content from input.
        let input_text = match &input {
            DAGData::Text(s) => s.clone(),
            DAGData::STTResult(r) => r.transcript.clone(),
            DAGData::Json(j) => j
                .get("content")
                .or_else(|| j.get("text"))
                .or_else(|| j.get("transcript"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| j.to_string()),
            other => {
                return Err(DAGError::InvalidInput {
                    node_id: self.id.clone(),
                    expected: "text, stt_result, or json".to_string(),
                    got: other.type_name().to_string(),
                });
            }
        };

        if input_text.trim().is_empty() {
            return Ok(DAGData::Empty);
        }

        // Per-connection API key from the DAG context takes priority (matches the
        // old node behavior); otherwise the client falls back to config/env.
        let response = self
            .client
            .complete(
                &ctx.stream_id,
                &input_text,
                ctx.api_key.as_deref(),
                &ctx.cancel_token,
                None,
            )
            .await
            .map_err(map_llm_error)?;

        // A plain text completion (no tool calls) flows downstream as `Text` so it chains directly
        // into a TTS / text-output / another LLM node (e.g. STT → translate(LLM) → TTS). Tool-call
        // responses keep the structured `Json` envelope so a router/transform can act on them.
        if response.tool_calls.is_empty() && !response.content.is_empty() {
            return Ok(DAGData::Text(response.content));
        }

        let response_json = serde_json::json!({
            "id": response.id,
            "model": response.model,
            "content": if response.content.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(response.content) },
            "role": "assistant",
            "finish_reason": response.finish_reason,
            "tool_calls": if response.tool_calls.is_empty() { serde_json::Value::Null } else { serde_json::json!(response.tool_calls) },
            "usage": response.usage,
            "streaming": self.streaming,
        });

        Ok(DAGData::Json(response_json))
    }

    /// Streaming override: stream LLM tokens and emit each COMPLETE sentence as a `DAGData::Text`
    /// chunk the moment it finishes — so a downstream TTS node can start speaking sentence 1 while
    /// the model is still generating sentence 2 (the core real-time win). Requires the node's
    /// client to be in streaming mode (`streaming: true`); otherwise the provider returns the whole
    /// completion at once and this emits it as a single chunk. Honors `ctx.cancel_token` for barge-in.
    async fn execute_streaming(
        &self,
        mut inputs: mpsc::Receiver<DAGData>,
        ctx: &mut DAGContext,
        outputs: mpsc::Sender<DAGData>,
    ) -> DAGResult<()> {
        while let Some(input) = inputs.recv().await {
            if ctx.cancel_token.is_cancelled() {
                break;
            }
            let input_text = match &input {
                DAGData::Text(s) => s.clone(),
                DAGData::STTResult(r) => r.transcript.clone(),
                DAGData::Empty => continue,
                other => {
                    return Err(DAGError::UnsupportedDataType {
                        expected: "text".to_string(),
                        actual: other.type_name().to_string(),
                    });
                }
            };
            if input_text.trim().is_empty() {
                continue;
            }

            // Bridge the synchronous per-token callback to this async loop.
            let (tok_tx, mut tok_rx) = mpsc::unbounded_channel::<String>();
            let on_token: TokenCallback = Arc::new(move |t: &str| {
                let _ = tok_tx.send(t.to_string());
            });

            let mut agg = SentenceAggregator::default();
            let mut emitted_any = false;
            let completion = self.client.complete(
                &ctx.stream_id,
                &input_text,
                ctx.api_key.as_deref(),
                &ctx.cancel_token,
                Some(on_token),
            );
            tokio::pin!(completion);

            // Drain tokens CONCURRENTLY with the completion: flush each finished sentence asap.
            let completion_result = loop {
                tokio::select! {
                    res = &mut completion => break res,
                    Some(tok) = tok_rx.recv() => {
                        for sentence in agg.push_str(&tok) {
                            // Barge-in/teardown mid-burst: stop emitting stale
                            // sentences downstream.
                            if ctx.cancel_token.is_cancelled() {
                                return Ok(());
                            }
                            emitted_any = true;
                            if outputs.send(DAGData::Text(sentence)).await.is_err() {
                                return Ok(()); // downstream gone
                            }
                        }
                    }
                }
            };
            // Catch any tokens that landed between the last poll and completion.
            while let Ok(tok) = tok_rx.try_recv() {
                for sentence in agg.push_str(&tok) {
                    if ctx.cancel_token.is_cancelled() {
                        return Ok(());
                    }
                    emitted_any = true;
                    if outputs.send(DAGData::Text(sentence)).await.is_err() {
                        return Ok(());
                    }
                }
            }
            let mut tail = agg.flush();
            match completion_result {
                Ok(response) => {
                    // Non-streaming providers deliver content only in the final response, not as
                    // deltas — fall back to the full content so the chain still produces output.
                    if !emitted_any && tail.is_none() && !response.content.is_empty() {
                        tail = Some(response.content);
                    }
                }
                Err(e) => return Err(map_llm_error(e)),
            }
            if let Some(tail) = tail
                && !tail.trim().is_empty()
            {
                let _ = outputs.send(DAGData::Text(tail)).await;
            }
        }
        Ok(())
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_message_constructors() {
        let system = ChatMessage::system("You are helpful");
        assert_eq!(system.role, MessageRole::System);
        assert_eq!(system.content, Some("You are helpful".to_string()));

        let user = ChatMessage::user("Hello");
        assert_eq!(user.role, MessageRole::User);

        let assistant = ChatMessage::assistant("Hi there!");
        assert_eq!(assistant.role, MessageRole::Assistant);
    }

    #[test]
    fn streaming_sentence_chunking_via_shared_aggregator() {
        // The DAG path now uses the SAME aggregator as the conversation pump.
        // Assert the streaming-relevant semantics at this call site.
        let mut agg = SentenceAggregator::default();

        // No terminator yet → nothing emitted, text retained for later.
        assert!(agg.push_str("the quick brown").is_empty());
        assert_eq!(agg.flush().as_deref(), Some("the quick brown"));

        // Finished sentence emits on lookahead; the partial stays buffered.
        let mut agg = SentenceAggregator::default();
        let out = agg.push_str("Hello there. How are y");
        assert_eq!(out, vec!["Hello there."]);
        assert_eq!(agg.flush().as_deref(), Some("How are y"));

        // Devanagari danda flushes immediately (unambiguous, no lookahead).
        let mut agg = SentenceAggregator::default();
        assert_eq!(agg.push_str("नमस्ते। आप"), vec!["नमस्ते।"]);

        // Each finished sentence emits separately (per-sentence TTS chunks).
        let mut agg = SentenceAggregator::default();
        assert_eq!(agg.push_str("One. Two! Three? tail"), vec!["One.", "Two!", "Three?"]);
        assert_eq!(agg.flush().as_deref(), Some("tail"));

        // The lookahead guards: decimals/abbreviations never split mid-token.
        let mut agg = SentenceAggregator::default();
        assert_eq!(agg.push_str("Pi is 3.14. Mr. Tau wins. End"), vec!["Pi is 3.14.", "Mr. Tau wins."]);
    }

    #[test]
    fn test_llm_config_default() {
        let config = LlmEndpointConfig::default();
        assert_eq!(config.base_url, "https://api.openai.com/v1");
        assert_eq!(config.model, "gpt-4o");
        assert!(!config.streaming);
    }

    #[test]
    fn test_llm_config_custom_base_url() {
        let config = LlmEndpointConfig {
            base_url: "http://localhost:11434/v1".to_string(),
            model: "llama3".to_string(),
            ..Default::default()
        };

        let node = LlmEndpointNode::new("test", config);
        assert_eq!(
            node.chat_completions_url(),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[test]
    fn test_tool_definition() {
        let tool = ToolDefinition::function(
            "get_weather",
            "Get weather for a location",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "location": { "type": "string" }
                },
                "required": ["location"]
            }),
        );

        assert_eq!(tool.tool_type, "function");
        assert_eq!(tool.function.name, "get_weather");
    }

    #[test]
    fn test_chat_completion_request_serialization() {
        let request = ChatCompletionRequest {
            model: "gpt-4o".to_string(),
            messages: vec![
                ChatMessage::system("You are helpful"),
                ChatMessage::user("Hello"),
            ],
            temperature: Some(0.7),
            stream: Some(false),
            ..Default::default()
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("gpt-4o"));
        assert!(json.contains("system"));
        assert!(json.contains("user"));
    }

    #[test]
    fn test_response_format_serialization() {
        let format = ResponseFormat::JsonObject;
        let json = serde_json::to_string(&format).unwrap();
        assert!(json.contains("json_object"));

        let schema_format = ResponseFormat::JsonSchema {
            json_schema: serde_json::json!({
                "name": "response",
                "schema": { "type": "object" }
            }),
        };
        let schema_json = serde_json::to_string(&schema_format).unwrap();
        assert!(schema_json.contains("json_schema"));
    }
}
