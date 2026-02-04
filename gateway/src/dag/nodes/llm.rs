//! LLM endpoint node for OpenAI-compatible API integration
//!
//! This module provides a dedicated LLM endpoint node that supports:
//! - Custom base URLs (OpenAI, Azure, Ollama, vLLM, LiteLLM, etc.)
//! - OpenAI-compatible request/response format
//! - Streaming support with Server-Sent Events (SSE)
//! - Conversation history management
//! - Tool/Function calling
//! - Agent/Assistant attachment
//! - System prompts and context injection

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, info, instrument, warn};

use super::{DAGData, DAGNode, NodeCapability};
use crate::dag::context::DAGContext;
use crate::dag::error::{DAGError, DAGResult};

/// Whitelisted environment variables that can be accessed via ${VAR} syntax
///
/// This prevents exposure of sensitive system environment variables like
/// database credentials, internal service tokens, or system paths.
/// Only AI provider API keys and related configuration should be allowed.
const ALLOWED_ENV_VARS: &[&str] = &[
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
];

/// OpenAI-compatible message role
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
    Function,
}

impl Default for MessageRole {
    fn default() -> Self {
        Self::User
    }
}

/// OpenAI-compatible message format
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
    /// Create a system message
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

    /// Create a user message
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

    /// Create an assistant message
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

    /// Create a tool response message
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

/// Tool call from assistant response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

/// Function call details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Tool definition for function calling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDefinition,
}

impl ToolDefinition {
    /// Create a new function tool
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

/// Function definition for tools
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

/// Response format specification
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    Text,
    JsonObject,
    JsonSchema { json_schema: serde_json::Value },
}

/// OpenAI-compatible chat completion request
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
    /// Extra parameters for provider-specific features
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

/// OpenAI-compatible chat completion response
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

/// Chat completion choice
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
    pub logprobs: Option<serde_json::Value>,
}

/// Streaming chat completion chunk
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

/// Streaming choice delta
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct StreamChoice {
    pub index: u32,
    pub delta: StreamDelta,
    pub finish_reason: Option<String>,
}

/// Streaming delta content
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct StreamDelta {
    pub role: Option<MessageRole>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

/// Streaming tool call delta
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCallDelta {
    pub index: u32,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub call_type: Option<String>,
    pub function: Option<FunctionCallDelta>,
}

/// Streaming function call delta
#[derive(Debug, Clone, Deserialize)]
pub struct FunctionCallDelta {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

/// Token usage statistics
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

/// LLM endpoint configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmEndpointConfig {
    /// Custom base URL (e.g., "https://api.openai.com/v1", "http://localhost:11434/v1")
    pub base_url: String,
    /// Model identifier
    pub model: String,
    /// API key (can use ${ENV_VAR} syntax)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Organization ID (OpenAI)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    /// System prompt
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Temperature (0.0 - 2.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Top P (0.0 - 1.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Maximum tokens to generate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Enable streaming responses
    #[serde(default)]
    pub streaming: bool,
    /// Tool definitions for function calling
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    /// Tool choice strategy
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    /// Response format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    /// Stop sequences
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    /// Request timeout in milliseconds
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Maximum conversation history to maintain
    #[serde(default = "default_max_history")]
    pub max_history: usize,
    /// Extra headers
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Agent/Assistant ID (for OpenAI Assistants API)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant_id: Option<String>,
    /// Thread ID (for OpenAI Assistants API)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Extra provider-specific parameters
    #[serde(default)]
    pub extra: HashMap<String, serde_json::Value>,
}

fn default_timeout_ms() -> u64 {
    60000
}

fn default_max_history() -> usize {
    20
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

/// Conversation history for maintaining context
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
        // Trim history to max_messages (keep system message if present)
        while self.messages.len() > self.max_messages {
            // Find first non-system message to remove
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

/// LLM endpoint node for OpenAI-compatible APIs
///
/// Supports:
/// - Any OpenAI-compatible API (OpenAI, Azure, Ollama, vLLM, LiteLLM, etc.)
/// - Streaming and non-streaming responses
/// - Function/Tool calling
/// - Conversation history management
/// - Agent/Assistant attachment
#[derive(Clone)]
pub struct LlmEndpointNode {
    id: String,
    config: LlmEndpointConfig,
    client: reqwest::Client,
    /// Per-stream conversation history, keyed by stream_id
    /// This prevents data leakage between different users/sessions
    histories: Arc<RwLock<HashMap<String, ConversationHistory>>>,
}

impl std::fmt::Debug for LlmEndpointNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmEndpointNode")
            .field("id", &self.id)
            .field("base_url", &self.config.base_url)
            .field("model", &self.config.model)
            .field("streaming", &self.config.streaming)
            .finish()
    }
}

impl LlmEndpointNode {
    /// Create a new LLM endpoint node
    pub fn new(id: impl Into<String>, config: LlmEndpointConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .pool_max_idle_per_host(10)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            id: id.into(),
            histories: Arc::new(RwLock::new(HashMap::new())),
            config,
            client,
        }
    }

    /// Create from JSON config
    pub fn from_json(id: impl Into<String>, config: serde_json::Value) -> Result<Self, String> {
        let config: LlmEndpointConfig =
            serde_json::from_value(config).map_err(|e| format!("Invalid LLM config: {}", e))?;
        Ok(Self::new(id, config))
    }

    /// Get the chat completions endpoint URL
    fn chat_completions_url(&self) -> String {
        let base = self.config.base_url.trim_end_matches('/');
        format!("{}/chat/completions", base)
    }

    /// Get API key from config or environment
    ///
    /// Priority: per-connection API key > config API key > env var
    ///
    /// Environment variable references use ${VAR} syntax and are restricted
    /// to whitelisted AI provider keys only (see ALLOWED_ENV_VARS).
    fn resolve_api_key(&self, ctx: &DAGContext) -> Option<String> {
        // Priority 1: per-connection API key from DAGContext
        if let Some(key) = &ctx.api_key {
            return Some(key.clone());
        }

        // Priority 2: config API key (may be literal or env var reference)
        if let Some(key) = &self.config.api_key {
            // Check for environment variable syntax ${VAR}
            if key.starts_with("${") && key.ends_with('}') {
                let var_name = &key[2..key.len() - 1];

                // Security: Only allow whitelisted environment variables
                if !ALLOWED_ENV_VARS.contains(&var_name) {
                    warn!(
                        node_id = %self.id,
                        var_name = %var_name,
                        "Blocked access to non-whitelisted environment variable"
                    );
                    return None;
                }

                return std::env::var(var_name).ok();
            }
            return Some(key.clone());
        }

        // Priority 3: Default to OPENAI_API_KEY from environment
        std::env::var("OPENAI_API_KEY").ok()
    }

    /// Build the chat completion request
    fn build_request(&self, messages: Vec<ChatMessage>) -> ChatCompletionRequest {
        let mut request = ChatCompletionRequest {
            model: self.config.model.clone(),
            messages,
            temperature: self.config.temperature,
            top_p: self.config.top_p,
            max_tokens: self.config.max_tokens,
            stream: Some(self.config.streaming),
            tools: self.config.tools.clone(),
            tool_choice: self.config.tool_choice.clone(),
            response_format: self.config.response_format.clone(),
            stop: self.config.stop.clone(),
            extra: self.config.extra.clone(),
            ..Default::default()
        };

        // Add max_completion_tokens if max_tokens not set (newer API)
        if self.config.max_tokens.is_none() {
            request.max_completion_tokens = Some(4096);
        }

        request
    }

    /// Execute non-streaming request
    #[instrument(skip(self, ctx), fields(node_id = %self.id, model = %self.config.model))]
    async fn execute_sync(&self, input: &str, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        // Get or create per-stream history (isolated per user/session)
        let mut histories = self.histories.write().await;
        let history = histories
            .entry(ctx.stream_id.clone())
            .or_insert_with(|| ConversationHistory::new(self.config.max_history));

        // Add system prompt if first message
        if history.messages().is_empty() {
            if let Some(system_prompt) = &self.config.system_prompt {
                history.add(ChatMessage::system(system_prompt));
            }
        }

        // Add user message
        history.add(ChatMessage::user(input));

        // Build request with full history
        let request = self.build_request(history.messages().to_vec());

        // Release lock before making HTTP request
        drop(histories);

        debug!(
            node_id = %self.id,
            url = %self.chat_completions_url(),
            messages_count = request.messages.len(),
            "Sending chat completion request"
        );

        // Build HTTP request
        let mut http_request = self
            .client
            .post(&self.chat_completions_url())
            .header("Content-Type", "application/json")
            .json(&request);

        // Add authorization
        if let Some(api_key) = self.resolve_api_key(ctx) {
            http_request = http_request.header("Authorization", format!("Bearer {}", api_key));
        }

        // Add organization header if set
        if let Some(org) = &self.config.organization {
            http_request = http_request.header("OpenAI-Organization", org);
        }

        // Add custom headers
        for (key, value) in &self.config.headers {
            http_request = http_request.header(key, value);
        }

        // Send request
        let response = http_request
            .send()
            .await
            .map_err(|e| DAGError::LlmEndpointError {
                provider: self.config.base_url.clone(),
                model: self.config.model.clone(),
                error: format!("Request failed: {}", e),
            })?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(DAGError::LlmEndpointError {
                provider: self.config.base_url.clone(),
                model: self.config.model.clone(),
                error: format!("HTTP {} - {}", status, error_text),
            });
        }

        // Parse response
        let completion: ChatCompletionResponse =
            response
                .json()
                .await
                .map_err(|e| DAGError::LlmEndpointError {
                    provider: self.config.base_url.clone(),
                    model: self.config.model.clone(),
                    error: format!("Failed to parse response: {}", e),
                })?;

        // Extract assistant message
        let assistant_message = completion
            .choices
            .first()
            .map(|c| c.message.clone())
            .ok_or_else(|| DAGError::LlmEndpointError {
                provider: self.config.base_url.clone(),
                model: self.config.model.clone(),
                error: "No choices in response".to_string(),
            })?;

        // Add assistant response to history (re-acquire lock)
        {
            let mut histories = self.histories.write().await;
            if let Some(history) = histories.get_mut(&ctx.stream_id) {
                history.add(assistant_message.clone());
            }
        }

        // Build response JSON
        let response_json = serde_json::json!({
            "id": completion.id,
            "model": completion.model,
            "content": assistant_message.content,
            "role": "assistant",
            "finish_reason": completion.choices.first().and_then(|c| c.finish_reason.clone()),
            "tool_calls": assistant_message.tool_calls,
            "usage": completion.usage,
        });

        info!(
            node_id = %self.id,
            completion_id = %completion.id,
            tokens = ?completion.usage.as_ref().map(|u| u.total_tokens),
            "Chat completion received"
        );

        Ok(DAGData::Json(response_json))
    }

    /// Execute streaming request
    #[instrument(skip(self, ctx), fields(node_id = %self.id, model = %self.config.model))]
    async fn execute_streaming(&self, input: &str, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        // Get or create per-stream history (isolated per user/session)
        let mut histories = self.histories.write().await;
        let history = histories
            .entry(ctx.stream_id.clone())
            .or_insert_with(|| ConversationHistory::new(self.config.max_history));

        // Add system prompt if first message
        if history.messages().is_empty() {
            if let Some(system_prompt) = &self.config.system_prompt {
                history.add(ChatMessage::system(system_prompt));
            }
        }

        // Add user message
        history.add(ChatMessage::user(input));

        // Build request with streaming enabled
        let mut request = self.build_request(history.messages().to_vec());
        request.stream = Some(true);

        // Release lock before making HTTP request
        drop(histories);

        debug!(
            node_id = %self.id,
            url = %self.chat_completions_url(),
            messages_count = request.messages.len(),
            "Sending streaming chat completion request"
        );

        // Build HTTP request
        let mut http_request = self
            .client
            .post(&self.chat_completions_url())
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&request);

        // Add authorization
        if let Some(api_key) = self.resolve_api_key(ctx) {
            http_request = http_request.header("Authorization", format!("Bearer {}", api_key));
        }

        // Add organization header if set
        if let Some(org) = &self.config.organization {
            http_request = http_request.header("OpenAI-Organization", org);
        }

        // Add custom headers
        for (key, value) in &self.config.headers {
            http_request = http_request.header(key, value);
        }

        // Send request
        let response = http_request
            .send()
            .await
            .map_err(|e| DAGError::LlmEndpointError {
                provider: self.config.base_url.clone(),
                model: self.config.model.clone(),
                error: format!("Request failed: {}", e),
            })?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(DAGError::LlmEndpointError {
                provider: self.config.base_url.clone(),
                model: self.config.model.clone(),
                error: format!("HTTP {} - {}", status, error_text),
            });
        }

        // Process SSE stream
        let mut stream = response.bytes_stream();
        let mut full_content = String::new();
        let mut completion_id = String::new();
        let mut model_name = String::new();
        let mut finish_reason: Option<String> = None;
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut tool_call_builders: HashMap<u32, (String, String, String, String)> = HashMap::new();

        // Byte buffer for handling incomplete UTF-8 sequences at chunk boundaries
        let mut byte_buffer: Vec<u8> = Vec::new();
        // String buffer for incomplete SSE lines
        let mut buffer = String::new();

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| DAGError::LlmEndpointError {
                provider: self.config.base_url.clone(),
                model: self.config.model.clone(),
                error: format!("Stream error: {}", e),
            })?;

            // Append new bytes to buffer
            byte_buffer.extend_from_slice(&chunk);

            // Find the last valid UTF-8 boundary to avoid corrupting multi-byte characters
            let valid_len = find_utf8_boundary(&byte_buffer);
            if valid_len == 0 {
                // No complete UTF-8 sequence yet, wait for more data
                continue;
            }

            // Decode the valid portion
            let valid_bytes = &byte_buffer[..valid_len];
            match std::str::from_utf8(valid_bytes) {
                Ok(text) => {
                    buffer.push_str(text);
                    // Remove processed bytes, keeping any incomplete trailing sequence
                    byte_buffer = byte_buffer[valid_len..].to_vec();
                }
                Err(e) => {
                    // This shouldn't happen if find_utf8_boundary works correctly
                    warn!(
                        node_id = %self.id,
                        error = %e,
                        "UTF-8 decode error in SSE stream, using lossy conversion"
                    );
                    buffer.push_str(&String::from_utf8_lossy(valid_bytes));
                    byte_buffer = byte_buffer[valid_len..].to_vec();
                }
            }

            // Process complete lines
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
                                }
                                if choice.finish_reason.is_some() {
                                    finish_reason = choice.finish_reason.clone();
                                }

                                // Handle streaming tool calls
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
                            warn!(
                                node_id = %self.id,
                                error = %e,
                                data = %data,
                                "Failed to parse streaming chunk"
                            );
                        }
                    }
                }
            }
        }

        // Build final tool calls from accumulated deltas
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

        // Build assistant message for history
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

        // Add to history (re-acquire lock)
        {
            let mut histories = self.histories.write().await;
            if let Some(history) = histories.get_mut(&ctx.stream_id) {
                history.add(assistant_message);
            }
        }

        // Build response JSON
        let response_json = serde_json::json!({
            "id": completion_id,
            "model": model_name,
            "content": if full_content.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(full_content) },
            "role": "assistant",
            "finish_reason": finish_reason,
            "tool_calls": if tool_calls.is_empty() { serde_json::Value::Null } else { serde_json::json!(tool_calls) },
            "streaming": true,
        });

        info!(
            node_id = %self.id,
            completion_id = %completion_id,
            "Streaming chat completion finished"
        );

        Ok(DAGData::Json(response_json))
    }

    /// Add a tool result to the conversation for a specific stream
    pub async fn add_tool_result(&self, stream_id: &str, tool_call_id: &str, result: &str) {
        let mut histories = self.histories.write().await;
        if let Some(history) = histories.get_mut(stream_id) {
            history.add(ChatMessage::tool(tool_call_id, result));
        }
    }

    /// Clear conversation history for a specific stream
    pub async fn clear_history(&self, stream_id: &str) {
        let mut histories = self.histories.write().await;
        if let Some(history) = histories.get_mut(stream_id) {
            history.clear();
        }
    }

    /// Remove conversation history for a specific stream (cleanup on disconnect)
    pub async fn remove_history(&self, stream_id: &str) {
        let mut histories = self.histories.write().await;
        histories.remove(stream_id);
    }

    /// Get the number of streams with active history (for monitoring)
    pub async fn active_stream_count(&self) -> usize {
        self.histories.read().await.len()
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
            if self.config.streaming {
                NodeCapability::Streaming
            } else {
                NodeCapability::TextOutput
            },
        ]
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        // Extract text content from input
        let input_text = match &input {
            DAGData::Text(s) => s.clone(),
            DAGData::STTResult(r) => r.transcript.clone(),
            DAGData::Json(j) => {
                // Try to extract "content" or "text" field, or stringify
                j.get("content")
                    .or_else(|| j.get("text"))
                    .or_else(|| j.get("transcript"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| j.to_string())
            }
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

        if self.config.streaming {
            self.execute_streaming(&input_text, ctx).await
        } else {
            self.execute_sync(&input_text, ctx).await
        }
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

/// Find the last valid UTF-8 boundary in a byte slice
///
/// This function returns the length of the longest prefix of `bytes` that is
/// valid UTF-8. This is useful when processing streaming data where a multi-byte
/// UTF-8 character might be split across chunks.
///
/// # UTF-8 Encoding Details
/// - 1-byte chars: 0xxxxxxx (ASCII, 0x00-0x7F)
/// - 2-byte chars: 110xxxxx 10xxxxxx (0xC0-0xDF, 0x80-0xBF)
/// - 3-byte chars: 1110xxxx 10xxxxxx 10xxxxxx (0xE0-0xEF, 0x80-0xBF, 0x80-0xBF)
/// - 4-byte chars: 11110xxx 10xxxxxx 10xxxxxx 10xxxxxx (0xF0-0xF7, ...)
fn find_utf8_boundary(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }

    // Try to decode the entire buffer
    match std::str::from_utf8(bytes) {
        Ok(_) => return bytes.len(),
        Err(e) => {
            // e.valid_up_to() gives us the byte offset of the first error
            let valid_len = e.valid_up_to();

            // If error_len is Some, it's an invalid sequence that can't be completed
            // If error_len is None, it's an incomplete sequence at the end
            if valid_len > 0 {
                return valid_len;
            }

            // The first byte itself is problematic - check if it's a leading byte
            // of a multi-byte sequence that needs more data
            let first_byte = bytes[0];
            if first_byte >= 0x80 {
                // It's a continuation byte or leading byte without enough data
                // Return 0 to wait for more data
                return 0;
            }

            // Invalid UTF-8 at the start - skip one byte
            return 1;
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
    fn test_conversation_history() {
        let mut history = ConversationHistory::new(5);

        history.add(ChatMessage::system("System prompt"));
        history.add(ChatMessage::user("Hello"));
        history.add(ChatMessage::assistant("Hi"));
        history.add(ChatMessage::user("How are you?"));
        history.add(ChatMessage::assistant("I'm good"));
        history.add(ChatMessage::user("Great"));

        // Should have trimmed to 5, but kept system message
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

    #[test]
    fn test_utf8_boundary_ascii() {
        // Pure ASCII should work fully
        let bytes = b"Hello, World!";
        assert_eq!(find_utf8_boundary(bytes), bytes.len());
    }

    #[test]
    fn test_utf8_boundary_complete_multibyte() {
        // Complete 2-byte UTF-8: é (U+00E9) = 0xC3 0xA9
        let bytes = vec![0xC3, 0xA9];
        assert_eq!(find_utf8_boundary(&bytes), 2);

        // Complete 3-byte UTF-8: 中 (U+4E2D) = 0xE4 0xB8 0xAD
        let bytes = vec![0xE4, 0xB8, 0xAD];
        assert_eq!(find_utf8_boundary(&bytes), 3);

        // Complete 4-byte UTF-8: 😀 (U+1F600) = 0xF0 0x9F 0x98 0x80
        let bytes = vec![0xF0, 0x9F, 0x98, 0x80];
        assert_eq!(find_utf8_boundary(&bytes), 4);
    }

    #[test]
    fn test_utf8_boundary_incomplete_multibyte() {
        // Incomplete 2-byte: only first byte of é
        let bytes = vec![0xC3];
        assert_eq!(find_utf8_boundary(&bytes), 0);

        // Incomplete 3-byte: first 2 bytes of 中
        let bytes = vec![0xE4, 0xB8];
        assert_eq!(find_utf8_boundary(&bytes), 0);

        // Incomplete 4-byte: first 3 bytes of 😀
        let bytes = vec![0xF0, 0x9F, 0x98];
        assert_eq!(find_utf8_boundary(&bytes), 0);
    }

    #[test]
    fn test_utf8_boundary_mixed() {
        // "Hello" + incomplete 3-byte character
        let mut bytes = b"Hello".to_vec();
        bytes.extend_from_slice(&[0xE4, 0xB8]); // Incomplete 中
        assert_eq!(find_utf8_boundary(&bytes), 5); // Only "Hello" is valid

        // "Hello中" complete
        let mut bytes = b"Hello".to_vec();
        bytes.extend_from_slice(&[0xE4, 0xB8, 0xAD]);
        assert_eq!(find_utf8_boundary(&bytes), 8); // Full string is valid
    }

    #[test]
    fn test_utf8_boundary_empty() {
        let bytes: &[u8] = &[];
        assert_eq!(find_utf8_boundary(bytes), 0);
    }
}
