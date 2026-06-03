//! Feature-flag-free OpenAI-compatible LLM client (`LlmClient`).
//!
//! This module is the single, shared implementation of OpenAI-compatible
//! chat-completions logic (request/response types, SSE streaming, function/tool
//! calling, conversation history). It deliberately does **not** depend on the
//! `dag-routing` feature or any `crate::dag::*` types, so that:
//!
//! - The built-in conversation orchestrator (`crate::core::conversation`) can use
//!   it in the **default** build (no DAG feature required).
//! - The DAG `LlmEndpointNode` (`crate::dag::nodes::llm`) delegates to it, so there
//!   is exactly one place that talks to an LLM.
//!
//! # Why a separate module
//!
//! `dag/nodes/llm.rs` previously owned all of this logic, but it is gated behind
//! `feature = "dag-routing"`. The conversation loop (plan W-O2) must work without
//! that feature, so the transport-level logic lives here and the DAG node became a
//! thin adapter over `LlmClient`.

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, instrument, warn};

/// Whitelisted environment variables that can be accessed via `${VAR}` syntax.
///
/// Prevents exposure of sensitive system environment variables; only AI provider
/// API keys and related configuration are allowed.
pub const ALLOWED_ENV_VARS: &[&str] = &[
    // OpenAI
    "OPENAI_API_KEY",
    "OPENAI_ORG_ID",
    "OPENAI_BASE_URL",
    // Anthropic
    "ANTHROPIC_API_KEY",
    // Azure OpenAI
    "AZURE_OPENAI_API_KEY",
    "AZURE_OPENAI_ENDPOINT",
    // Google AI / Vertex
    "GOOGLE_AI_API_KEY",
    "GOOGLE_CLOUD_API_KEY",
    // Groq
    "GROQ_API_KEY",
    // Together AI
    "TOGETHER_API_KEY",
    // Perplexity
    "PERPLEXITY_API_KEY",
    // Mistral
    "MISTRAL_API_KEY",
    // Cohere
    "COHERE_API_KEY",
    // Fireworks
    "FIREWORKS_API_KEY",
    // Replicate
    "REPLICATE_API_TOKEN",
    // HuggingFace
    "HUGGINGFACE_API_KEY",
    "HF_TOKEN",
    // Local/Self-hosted (Ollama, vLLM, etc.)
    "LOCAL_LLM_API_KEY",
    "OLLAMA_API_KEY",
    "VLLM_API_KEY",
    // LiteLLM proxy
    "LITELLM_API_KEY",
    // Sarvam (OpenAI-compatible chat; Indic translation/LLM)
    "SARVAM_API_KEY",
];

/// Maximum number of bytes of streamed LLM content to accumulate before the
/// stream is treated as a runaway and terminated. 8 MiB is far beyond any
/// legitimate single completion and prevents an unbounded provider (or a
/// malicious mock) from exhausting memory.
const MAX_LLM_STREAM_CONTENT_BYTES: usize = 8 * 1024 * 1024;

fn default_timeout_ms() -> u64 {
    60000
}

fn default_max_history() -> usize {
    20
}

/// Error type for the LLM client. Self-contained (no DAG dependency).
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// Transport / provider error (network, HTTP non-2xx, parse failure).
    #[error("LLM endpoint error [{provider}/{model}]: {error}")]
    Endpoint {
        provider: String,
        model: String,
        error: String,
    },

    /// The request was cancelled (e.g. barge-in or session teardown).
    #[error("LLM request cancelled")]
    Cancelled,
}

pub type LlmResult<T> = Result<T, LlmError>;

// =============================================================================
// OpenAI-compatible wire types
// =============================================================================

/// OpenAI-compatible message role.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum MessageRole {
    System,
    #[default]
    User,
    Assistant,
    Tool,
    Function,
}


/// OpenAI-compatible message format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call: Option<FunctionCall>,
}

impl ChatMessage {
    /// Create a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: Some(content.into()),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            function_call: None,
        }
    }

    /// Create a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: Some(content.into()),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            function_call: None,
        }
    }

    /// Create an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: Some(content.into()),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            function_call: None,
        }
    }

    /// Create a tool response message.
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Tool,
            content: Some(content.into()),
            name: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            function_call: None,
        }
    }
}

/// Tool call from assistant response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

/// Function call details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Tool definition for function calling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDefinition,
}

impl ToolDefinition {
    /// Create a new function tool.
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: name.into(),
                description: description.into(),
                parameters,
                strict: None,
            },
        }
    }
}

/// Function definition for tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

/// Response format specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    Text,
    JsonObject,
    JsonSchema { json_schema: serde_json::Value },
}

/// OpenAI-compatible chat completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Extra parameters for provider-specific features.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Default for ChatCompletionRequest {
    fn default() -> Self {
        Self {
            model: "gpt-4o".to_string(),
            messages: Vec::new(),
            temperature: None,
            top_p: None,
            max_tokens: None,
            max_completion_tokens: None,
            stream: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            seed: None,
            stop: None,
            presence_penalty: None,
            frequency_penalty: None,
            user: None,
            extra: HashMap::new(),
        }
    }
}

/// OpenAI-compatible chat completion response.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: Option<Usage>,
    pub system_fingerprint: Option<String>,
}

/// Chat completion choice.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
    pub logprobs: Option<serde_json::Value>,
}

/// Streaming chat completion chunk.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<StreamChoice>,
    pub system_fingerprint: Option<String>,
    pub usage: Option<Usage>,
}

/// Streaming choice delta.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct StreamChoice {
    pub index: u32,
    pub delta: StreamDelta,
    pub finish_reason: Option<String>,
}

/// Streaming delta content.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct StreamDelta {
    pub role: Option<MessageRole>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

/// Streaming tool call delta.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCallDelta {
    pub index: u32,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub call_type: Option<String>,
    pub function: Option<FunctionCallDelta>,
}

/// Streaming function call delta.
#[derive(Debug, Clone, Deserialize)]
pub struct FunctionCallDelta {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

/// Token usage statistics.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens_details: Option<serde_json::Value>,
}

// =============================================================================
// Client configuration + per-session history
// =============================================================================

/// LLM client configuration. Field-compatible with the former
/// `LlmEndpointConfig` so the DAG node can keep deserializing the same JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmClientConfig {
    /// Custom base URL (e.g. `https://api.openai.com/v1`, `http://localhost:11434/v1`).
    pub base_url: String,
    /// Model identifier.
    pub model: String,
    /// API key (may use `${ENV_VAR}` syntax).
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

impl Default for LlmClientConfig {
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

/// Conversation history for maintaining context across turns.
#[derive(Debug, Clone, Default)]
pub struct ConversationHistory {
    messages: Vec<ChatMessage>,
    max_messages: usize,
}

impl ConversationHistory {
    pub fn new(max_messages: usize) -> Self {
        Self {
            messages: Vec::new(),
            max_messages,
        }
    }

    pub fn add(&mut self, message: ChatMessage) {
        self.messages.push(message);
        // Trim history to max_messages (keep system message if present).
        while self.messages.len() > self.max_messages {
            if let Some(idx) = self
                .messages
                .iter()
                .position(|m| m.role != MessageRole::System)
            {
                self.messages.remove(idx);
            } else {
                break;
            }
        }
    }

    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }
}

/// Result of an LLM turn.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    /// Completion id (provider-assigned), if any.
    pub id: String,
    /// Model that produced the response.
    pub model: String,
    /// Aggregated assistant text content (may be empty for a pure tool call).
    pub content: String,
    /// Finish reason (e.g. `stop`, `tool_calls`).
    pub finish_reason: Option<String>,
    /// Accumulated tool calls (if any).
    pub tool_calls: Vec<ToolCall>,
    /// Token usage (non-streaming only; providers rarely send it while streaming).
    pub usage: Option<Usage>,
}

/// A callback that receives streamed token deltas as they arrive.
///
/// This is what lets the conversation orchestrator pipe tokens straight to TTS
/// while the completion is still being generated.
pub type TokenCallback = Arc<dyn Fn(&str) + Send + Sync>;

/// Feature-flag-free OpenAI-compatible chat client.
///
/// Owns an HTTP client and a per-`session_id` conversation history map (so the
/// same client can serve many isolated sessions without leaking context).
#[derive(Clone)]
pub struct LlmClient {
    config: LlmClientConfig,
    client: reqwest::Client,
    /// Per-session conversation history, keyed by session/stream id. Isolated so
    /// one user's context never leaks into another's.
    histories: Arc<RwLock<HashMap<String, ConversationHistory>>>,
}

impl std::fmt::Debug for LlmClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmClient")
            .field("base_url", &self.config.base_url)
            .field("model", &self.config.model)
            .field("streaming", &self.config.streaming)
            .finish()
    }
}

impl LlmClient {
    /// Create a new client. Does **not** validate `base_url` for SSRF — callers
    /// that accept client-supplied URLs must validate separately (the DAG
    /// compiler and the conversation orchestrator both do).
    pub fn new(config: LlmClientConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .pool_max_idle_per_host(10)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            config,
            client,
            histories: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Access the configuration.
    pub fn config(&self) -> &LlmClientConfig {
        &self.config
    }

    /// The chat-completions endpoint URL.
    pub fn chat_completions_url(&self) -> String {
        let base = self.config.base_url.trim_end_matches('/');
        format!("{}/chat/completions", base)
    }

    /// Resolve the API key.
    ///
    /// Priority: per-call key > config key (literal or `${ENV_VAR}`) > `OPENAI_API_KEY`.
    pub fn resolve_api_key(&self, per_call: Option<&str>) -> Option<String> {
        if let Some(key) = per_call {
            return Some(key.to_string());
        }

        if let Some(key) = &self.config.api_key {
            if key.starts_with("${") && key.ends_with('}') {
                let var_name = &key[2..key.len() - 1];
                if !ALLOWED_ENV_VARS.contains(&var_name) {
                    warn!(
                        var_name = %var_name,
                        "Blocked access to non-whitelisted environment variable"
                    );
                    return None;
                }
                return std::env::var(var_name).ok();
            }
            return Some(key.clone());
        }

        std::env::var("OPENAI_API_KEY").ok()
    }

    fn build_request(&self, messages: Vec<ChatMessage>, stream: bool) -> ChatCompletionRequest {
        let mut request = ChatCompletionRequest {
            model: self.config.model.clone(),
            messages,
            temperature: self.config.temperature,
            top_p: self.config.top_p,
            max_tokens: self.config.max_tokens,
            stream: Some(stream),
            tools: self.config.tools.clone(),
            tool_choice: self.config.tool_choice.clone(),
            response_format: self.config.response_format.clone(),
            stop: self.config.stop.clone(),
            extra: self.config.extra.clone(),
            ..Default::default()
        };
        if self.config.max_tokens.is_none() {
            request.max_completion_tokens = Some(4096);
        }
        request
    }

    /// Append the user message to the session history (inserting the system
    /// prompt first when the history is empty) and return the full message list
    /// to send.
    async fn prepare_messages(&self, session_id: &str, input: &str) -> Vec<ChatMessage> {
        let mut histories = self.histories.write().await;
        let history = histories
            .entry(session_id.to_string())
            .or_insert_with(|| ConversationHistory::new(self.config.max_history));

        if history.messages().is_empty()
            && let Some(system_prompt) = &self.config.system_prompt {
                history.add(ChatMessage::system(system_prompt));
            }
        history.add(ChatMessage::user(input));
        history.messages().to_vec()
    }

    async fn record_assistant(&self, session_id: &str, message: ChatMessage) {
        let mut histories = self.histories.write().await;
        if let Some(history) = histories.get_mut(session_id) {
            history.add(message);
        }
    }

    fn apply_headers(&self, mut req: reqwest::RequestBuilder, api_key: Option<&str>) -> reqwest::RequestBuilder {
        if let Some(key) = api_key {
            req = req.header("Authorization", format!("Bearer {}", key));
        }
        if let Some(org) = &self.config.organization {
            req = req.header("OpenAI-Organization", org);
        }
        for (key, value) in &self.config.headers {
            req = req.header(key, value);
        }
        req
    }

    /// Run a single completion turn.
    ///
    /// The user `input` is appended to the per-session history; the assistant's
    /// reply is recorded back into it (so subsequent turns include prior ones).
    /// When `self.config.streaming` is true (or `stream` is forced) and an
    /// `on_token` callback is supplied, tokens are delivered incrementally as
    /// they arrive. `cancel` aborts the turn promptly (barge-in / teardown).
    pub async fn complete(
        &self,
        session_id: &str,
        input: &str,
        api_key: Option<&str>,
        cancel: &CancellationToken,
        on_token: Option<TokenCallback>,
    ) -> LlmResult<LlmResponse> {
        // Resolve the credential ONCE here (per-call key > config key, literal or `${ENV_VAR}` >
        // `OPENAI_API_KEY`). Without this, a `None` per-call key (e.g. a DAG node with no
        // per-connection key) skipped the config/`${ENV_VAR}` fallback entirely and sent an
        // unauthenticated request — the config `api_key` was silently ignored.
        let resolved = self.resolve_api_key(api_key);
        let api_key = resolved.as_deref();
        if self.config.streaming {
            self.complete_streaming(session_id, input, api_key, cancel, on_token)
                .await
        } else {
            self.complete_sync(session_id, input, api_key, cancel).await
        }
    }

    #[instrument(skip(self, cancel), fields(model = %self.config.model))]
    async fn complete_sync(
        &self,
        session_id: &str,
        input: &str,
        api_key: Option<&str>,
        cancel: &CancellationToken,
    ) -> LlmResult<LlmResponse> {
        let messages = self.prepare_messages(session_id, input).await;
        let request = self.build_request(messages, false);

        let http_request = self.apply_headers(
            self.client
                .post(self.chat_completions_url())
                .header("Content-Type", "application/json")
                .json(&request),
            api_key,
        );

        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(LlmError::Cancelled),
            r = http_request.send() => r.map_err(|e| self.endpoint_err(format!("Request failed: {}", e)))?,
        };

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(self.endpoint_err(format!("HTTP {} - {}", status, error_text)));
        }

        let completion: ChatCompletionResponse = response
            .json()
            .await
            .map_err(|e| self.endpoint_err(format!("Failed to parse response: {}", e)))?;

        let assistant_message = completion
            .choices
            .first()
            .map(|c| c.message.clone())
            .ok_or_else(|| self.endpoint_err("No choices in response".to_string()))?;

        self.record_assistant(session_id, assistant_message.clone())
            .await;

        Ok(LlmResponse {
            id: completion.id,
            model: completion.model,
            content: assistant_message.content.clone().unwrap_or_default(),
            finish_reason: completion.choices.first().and_then(|c| c.finish_reason.clone()),
            tool_calls: assistant_message.tool_calls.clone().unwrap_or_default(),
            usage: completion.usage,
        })
    }

    #[instrument(skip(self, cancel, on_token), fields(model = %self.config.model))]
    async fn complete_streaming(
        &self,
        session_id: &str,
        input: &str,
        api_key: Option<&str>,
        cancel: &CancellationToken,
        on_token: Option<TokenCallback>,
    ) -> LlmResult<LlmResponse> {
        let messages = self.prepare_messages(session_id, input).await;
        let request = self.build_request(messages, true);

        let http_request = self.apply_headers(
            self.client
                .post(self.chat_completions_url())
                .header("Content-Type", "application/json")
                .header("Accept", "text/event-stream")
                .json(&request),
            api_key,
        );

        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(LlmError::Cancelled),
            r = http_request.send() => r.map_err(|e| self.endpoint_err(format!("Request failed: {}", e)))?,
        };

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(self.endpoint_err(format!("HTTP {} - {}", status, error_text)));
        }

        let mut stream = response.bytes_stream();
        let mut full_content = String::new();
        let mut completion_id = String::new();
        let mut model_name = String::new();
        let mut finish_reason: Option<String> = None;
        let mut tool_call_builders: HashMap<u32, (String, String, String, String)> = HashMap::new();

        let mut byte_buffer: Vec<u8> = Vec::new();
        let mut buffer = String::new();

        loop {
            // Honor cancellation promptly so barge-in/teardown stops token
            // accumulation instead of draining the whole upstream response.
            let chunk_result = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    debug!("LLM stream cancelled");
                    return Err(LlmError::Cancelled);
                }
                next = stream.next() => match next {
                    Some(c) => c,
                    None => break,
                }
            };

            let chunk =
                chunk_result.map_err(|e| self.endpoint_err(format!("Stream error: {}", e)))?;

            byte_buffer.extend_from_slice(&chunk);

            let valid_len = find_utf8_boundary(&byte_buffer);
            if valid_len == 0 {
                continue;
            }

            let valid_bytes = &byte_buffer[..valid_len];
            match std::str::from_utf8(valid_bytes) {
                Ok(text) => {
                    buffer.push_str(text);
                    byte_buffer = byte_buffer[valid_len..].to_vec();
                }
                Err(e) => {
                    warn!(error = %e, "UTF-8 decode error in SSE stream, using lossy conversion");
                    buffer.push_str(&String::from_utf8_lossy(valid_bytes));
                    byte_buffer = byte_buffer[valid_len..].to_vec();
                }
            }

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.is_empty() || line == "data: [DONE]" {
                    continue;
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    match serde_json::from_str::<ChatCompletionChunk>(data) {
                        Ok(chunk) => {
                            if completion_id.is_empty() {
                                completion_id = chunk.id.clone();
                            }
                            if model_name.is_empty() {
                                model_name = chunk.model.clone();
                            }

                            for choice in &chunk.choices {
                                if let Some(content) = &choice.delta.content {
                                    full_content.push_str(content);
                                    if let Some(cb) = &on_token {
                                        cb(content);
                                    }
                                    if full_content.len() > MAX_LLM_STREAM_CONTENT_BYTES {
                                        warn!(
                                            limit = %MAX_LLM_STREAM_CONTENT_BYTES,
                                            "LLM stream exceeded max content size, terminating"
                                        );
                                        return Err(self.endpoint_err(format!(
                                            "Streaming response exceeded {} bytes",
                                            MAX_LLM_STREAM_CONTENT_BYTES
                                        )));
                                    }
                                }
                                if choice.finish_reason.is_some() {
                                    finish_reason = choice.finish_reason.clone();
                                }

                                if let Some(tc_deltas) = &choice.delta.tool_calls {
                                    for tc_delta in tc_deltas {
                                        let entry = tool_call_builders
                                            .entry(tc_delta.index)
                                            .or_insert_with(|| {
                                                (
                                                    String::new(),
                                                    String::new(),
                                                    String::new(),
                                                    String::new(),
                                                )
                                            });
                                        if let Some(id) = &tc_delta.id {
                                            entry.0 = id.clone();
                                        }
                                        if let Some(t) = &tc_delta.call_type {
                                            entry.1 = t.clone();
                                        }
                                        if let Some(f) = &tc_delta.function {
                                            if let Some(name) = &f.name {
                                                entry.2 = name.clone();
                                            }
                                            if let Some(args) = &f.arguments {
                                                entry.3.push_str(args);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, data = %data, "Failed to parse streaming chunk");
                        }
                    }
                }
            }
        }

        let mut tool_calls: Vec<ToolCall> = Vec::new();
        for (_, (id, call_type, name, arguments)) in tool_call_builders {
            if !id.is_empty() {
                tool_calls.push(ToolCall {
                    id,
                    call_type: if call_type.is_empty() {
                        "function".to_string()
                    } else {
                        call_type
                    },
                    function: FunctionCall { name, arguments },
                });
            }
        }

        let assistant_message = ChatMessage {
            role: MessageRole::Assistant,
            content: if full_content.is_empty() {
                None
            } else {
                Some(full_content.clone())
            },
            name: None,
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls.clone())
            },
            tool_call_id: None,
            function_call: None,
        };
        self.record_assistant(session_id, assistant_message).await;

        Ok(LlmResponse {
            id: completion_id,
            model: model_name,
            content: full_content,
            finish_reason,
            tool_calls,
            usage: None,
        })
    }

    /// Add a tool result message to a session's history.
    pub async fn add_tool_result(&self, session_id: &str, tool_call_id: &str, result: &str) {
        let mut histories = self.histories.write().await;
        if let Some(history) = histories.get_mut(session_id) {
            history.add(ChatMessage::tool(tool_call_id, result));
        }
    }

    /// Clear a session's conversation history (keeps the entry).
    pub async fn clear_history(&self, session_id: &str) {
        let mut histories = self.histories.write().await;
        if let Some(history) = histories.get_mut(session_id) {
            history.clear();
        }
    }

    /// Remove a session's conversation history entirely (cleanup on disconnect).
    pub async fn remove_history(&self, session_id: &str) {
        let mut histories = self.histories.write().await;
        histories.remove(session_id);
    }

    /// Number of sessions with active history (for monitoring/tests).
    pub async fn active_session_count(&self) -> usize {
        self.histories.read().await.len()
    }

    /// Snapshot of a session's history (for tests/inspection).
    pub async fn history_snapshot(&self, session_id: &str) -> Vec<ChatMessage> {
        let histories = self.histories.read().await;
        histories
            .get(session_id)
            .map(|h| h.messages().to_vec())
            .unwrap_or_default()
    }

    fn endpoint_err(&self, error: String) -> LlmError {
        LlmError::Endpoint {
            provider: self.config.base_url.clone(),
            model: self.config.model.clone(),
            error,
        }
    }
}

/// Find the length of the longest valid-UTF-8 prefix of `bytes`.
///
/// Used to avoid corrupting multi-byte UTF-8 characters split across SSE chunk
/// boundaries.
pub fn find_utf8_boundary(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(e) => {
            let valid_len = e.valid_up_to();
            if valid_len > 0 {
                return valid_len;
            }
            let first_byte = bytes[0];
            if first_byte >= 0x80 {
                return 0;
            }
            1
        }
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
    fn test_llm_config_default() {
        let config = LlmClientConfig::default();
        assert_eq!(config.base_url, "https://api.openai.com/v1");
        assert_eq!(config.model, "gpt-4o");
        assert!(!config.streaming);
    }

    #[test]
    fn test_chat_completions_url() {
        let config = LlmClientConfig {
            base_url: "http://localhost:11434/v1".to_string(),
            model: "llama3".to_string(),
            ..Default::default()
        };
        let client = LlmClient::new(config);
        assert_eq!(
            client.chat_completions_url(),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[test]
    fn test_conversation_history() {
        let mut history = ConversationHistory::new(5);
        history.add(ChatMessage::system("System prompt"));
        history.add(ChatMessage::user("Hello"));
        history.add(ChatMessage::assistant("Hi"));
        history.add(ChatMessage::user("How are you?"));
        history.add(ChatMessage::assistant("I'm good"));
        history.add(ChatMessage::user("Great"));
        assert!(history.messages.len() <= 6);
        assert_eq!(history.messages[0].role, MessageRole::System);
    }

    #[test]
    fn test_tool_definition() {
        let tool = ToolDefinition::function(
            "get_weather",
            "Get weather for a location",
            serde_json::json!({
                "type": "object",
                "properties": { "location": { "type": "string" } },
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
    }

    #[test]
    fn test_utf8_boundary_ascii() {
        let bytes = b"Hello, World!";
        assert_eq!(find_utf8_boundary(bytes), bytes.len());
    }

    #[test]
    fn test_utf8_boundary_complete_multibyte() {
        assert_eq!(find_utf8_boundary(&[0xC3, 0xA9]), 2);
        assert_eq!(find_utf8_boundary(&[0xE4, 0xB8, 0xAD]), 3);
        assert_eq!(find_utf8_boundary(&[0xF0, 0x9F, 0x98, 0x80]), 4);
    }

    #[test]
    fn test_utf8_boundary_incomplete_multibyte() {
        assert_eq!(find_utf8_boundary(&[0xC3]), 0);
        assert_eq!(find_utf8_boundary(&[0xE4, 0xB8]), 0);
        assert_eq!(find_utf8_boundary(&[0xF0, 0x9F, 0x98]), 0);
    }

    #[test]
    fn test_utf8_boundary_mixed() {
        let mut bytes = b"Hello".to_vec();
        bytes.extend_from_slice(&[0xE4, 0xB8]);
        assert_eq!(find_utf8_boundary(&bytes), 5);

        let mut bytes = b"Hello".to_vec();
        bytes.extend_from_slice(&[0xE4, 0xB8, 0xAD]);
        assert_eq!(find_utf8_boundary(&bytes), 8);
    }

    #[test]
    fn test_utf8_boundary_empty() {
        let bytes: &[u8] = &[];
        assert_eq!(find_utf8_boundary(bytes), 0);
    }
}
