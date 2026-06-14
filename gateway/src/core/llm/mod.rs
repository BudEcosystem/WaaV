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

pub mod adapter;
pub mod functions;
pub mod summarize;

pub use adapter::{AdapterKind, LlmAdapter, LlmStreamEvent, ReasoningEffort};
pub use functions::{
    CANCEL_ASYNC_TOOL_NAME, FunctionCallParams, FunctionHandler, FunctionRegistry,
    FunctionResult, RegistryItem, ToolLoopOptions, run_tool_loop,
};
pub use summarize::SummaryConfig;

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

/// D-G9: export wire-reported token usage as counters. Kinds beyond
/// prompt/completion come from the details objects (cached_tokens,
/// reasoning_tokens) and are emitted ONLY when the wire carried them.
fn export_usage_metrics(provider: &str, usage: &Usage) {
    use crate::core::metrics::bridge::count_llm_tokens;
    count_llm_tokens(provider, "prompt", usage.prompt_tokens as u64);
    count_llm_tokens(provider, "completion", usage.completion_tokens as u64);
    if let Some(d) = &usage.prompt_tokens_details {
        if let Some(n) = d.get("cached_tokens").and_then(|v| v.as_u64()) {
            count_llm_tokens(provider, "cache_read", n);
        }
        if let Some(n) = d.get("cache_creation_tokens").and_then(|v| v.as_u64()) {
            count_llm_tokens(provider, "cache_creation", n);
        }
    }
    if let Some(d) = &usage.completion_tokens_details
        && let Some(n) = d.get("reasoning_tokens").and_then(|v| v.as_u64())
    {
        count_llm_tokens(provider, "reasoning", n);
    }
}

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
    /// Which vendor wire format to speak (B-G1). `None` infers ONLY the two
    /// canonical hosts (`api.anthropic.com`, `generativelanguage.googleapis.com`)
    /// and otherwise defaults to OpenAI — an OpenAI-compatible proxy in front
    /// of Claude/Gemini speaks the OpenAI format, so the URL is never trusted
    /// beyond the canonical hosts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_kind: Option<AdapterKind>,
    /// D1: reasoning/thinking-effort dial. `None` = vendor default (no param
    /// emitted). Vendor-mapped + floor-clamped in `adapter.rs` (the cascade path
    /// applies the floor in `ConversationConfig::to_client_config`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
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
            provider_kind: None,
            reasoning_effort: None,
        }
    }
}

/// Conversation history for maintaining context across turns.
#[derive(Debug, Clone, Default)]
pub struct ConversationHistory {
    messages: Vec<ChatMessage>,
    max_messages: usize,
    /// Monotonic mutation counter (B-G6): bumped on every change so a
    /// read-modify-write (summarization) can detect a concurrent mutation
    /// and refuse to clobber it (review wf_d43814c3 #2 lost-update RMW).
    version: u64,
}

impl ConversationHistory {
    pub fn new(max_messages: usize) -> Self {
        Self {
            messages: Vec::new(),
            max_messages,
            version: 0,
        }
    }

    /// The current mutation version (B-G6 CAS).
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn add(&mut self, message: ChatMessage) {
        self.version = self.version.wrapping_add(1);
        self.messages.push(message);
        // Trim history to max_messages (keep system message if present).
        while self.messages.len() > self.max_messages {
            if let Some(idx) = self
                .messages
                .iter()
                .position(|m| m.role != MessageRole::System)
            {
                self.messages.remove(idx);
                // Pair-aware eviction (review wf_6783a4b3): the oldest
                // non-system slot must never be left on a tool RESULT whose
                // call was just (or previously) evicted — strict providers
                // 400 on orphan tool messages. Any tool message now at the
                // front is by definition an orphan: drop it with its call.
                while idx < self.messages.len()
                    && self.messages[idx].role == MessageRole::Tool
                {
                    self.messages.remove(idx);
                }
            } else {
                break;
            }
        }
    }

    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    pub fn clear(&mut self) {
        self.version = self.version.wrapping_add(1);
        self.messages.clear();
    }

    /// Replace the leading system message, or insert one at the front
    /// (B-G3 `role_message` persistence).
    pub fn upsert_system(&mut self, content: &str) {
        self.version = self.version.wrapping_add(1);
        match self.messages.first_mut() {
            Some(m) if m.role == MessageRole::System => {
                m.content = Some(content.to_string());
            }
            _ => self.messages.insert(0, ChatMessage::system(content)),
        }
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
    /// The vendor wire-format adapter (B-G1). Stateless `'static` unit —
    /// selected once at construction from `config.provider_kind` (or the
    /// canonical-host inference).
    adapter: &'static dyn LlmAdapter,
    /// Per-session conversation history, keyed by session/stream id. Isolated so
    /// one user's context never leaks into another's.
    histories: Arc<RwLock<HashMap<String, ConversationHistory>>>,
    /// Server-side tool registry (B-G4). When set, its definitions are
    /// advertised in EVERY request (merged after `config.tools`) — the same
    /// registry drives `run_tool_loop` after a tool-call response.
    functions: Option<Arc<functions::FunctionRegistry>>,
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

        let adapter = adapter::select_adapter(&config);
        Self {
            config,
            client,
            adapter,
            histories: Arc::new(RwLock::new(HashMap::new())),
            functions: None,
        }
    }

    /// Attach a server-side function registry (B-G4): its tools are
    /// advertised in every request and executed by [`run_tool_loop`].
    pub fn with_functions(mut self, registry: Arc<functions::FunctionRegistry>) -> Self {
        self.functions = Some(registry);
        self
    }

    /// The attached function registry, if any.
    pub fn functions(&self) -> Option<&Arc<functions::FunctionRegistry>> {
        self.functions.as_ref()
    }

    /// Clone this client speaking a DIFFERENT vendor wire format while
    /// SHARING the per-session histories (the canonical context is
    /// vendor-neutral, so a mid-session vendor switch re-renders the same
    /// conversation without poisoning it — B-G1).
    pub fn with_adapter(&self, kind: AdapterKind) -> Self {
        let mut config = self.config.clone();
        config.provider_kind = Some(kind);
        Self {
            adapter: adapter::adapter_for(kind),
            config,
            client: self.client.clone(),
            histories: Arc::clone(&self.histories),
            functions: self.functions.clone(),
        }
    }

    /// S1/S2 (REALTIME_REASONING.md §5): clone this client as a second TIER —
    /// a different model/endpoint/effort — while SHARING the per-session history
    /// (the canonical context is vendor-neutral, so the fast tier and the slow
    /// reasoning tier see the SAME conversation; an escalated turn continues it
    /// rather than starting fresh). Overrides that are `None` inherit the base.
    pub fn with_tier_overrides(
        &self,
        model: String,
        base_url: Option<String>,
        api_key: Option<String>,
        provider_kind: Option<AdapterKind>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Self {
        let mut config = self.config.clone();
        config.model = model;
        if let Some(b) = base_url {
            config.base_url = b;
        }
        if let Some(k) = api_key {
            config.api_key = Some(k);
        }
        // The reasoning tier may use its own vendor; otherwise inherit.
        config.provider_kind = provider_kind.or(config.provider_kind);
        config.reasoning_effort = reasoning_effort;
        let kind = adapter::select_adapter(&config).kind();
        Self {
            adapter: adapter::adapter_for(kind),
            config,
            client: self.client.clone(),
            histories: Arc::clone(&self.histories),
            functions: self.functions.clone(),
        }
    }

    /// The active vendor adapter kind.
    pub fn adapter_kind(&self) -> AdapterKind {
        self.adapter.kind()
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
    /// Priority: per-call key > config key (literal or `${ENV_VAR}`) > the
    /// active vendor's default env var (`OPENAI_API_KEY` /
    /// `ANTHROPIC_API_KEY` / `GOOGLE_AI_API_KEY`).
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

        std::env::var(self.adapter.default_env_key()).ok()
    }

    /// Render a vendor request via the adapter and apply the operator's extra
    /// config headers (vendor auth/version headers come from the adapter).
    fn build_http_request(
        &self,
        messages: &[ChatMessage],
        stream: bool,
        api_key: Option<&str>,
    ) -> reqwest::RequestBuilder {
        // B-G4: advertise registry tools in every request. Config-declared
        // tools come first; registry definitions are appended (and the
        // builtin cancel tool rides along while async tools exist).
        let cfg_with_tools;
        let cfg = match &self.functions {
            Some(reg) if !reg.is_empty() => {
                let mut tools = self.config.tools.clone().unwrap_or_default();
                tools.extend(reg.request_tools());
                let mut c = self.config.clone();
                c.tools = Some(tools);
                cfg_with_tools = c;
                &cfg_with_tools
            }
            _ => &self.config,
        };
        let rendered = self.adapter.render_request(messages, cfg, stream, api_key);
        let mut req = self
            .client
            .post(&rendered.url)
            .header("Content-Type", "application/json");
        if stream {
            req = req.header("Accept", "text/event-stream");
        }
        for (k, v) in &rendered.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        for (k, v) in &self.config.headers {
            req = req.header(k, v);
        }
        req.json(&rendered.body)
    }

    /// Build the message list to send. `staged = false` (classic): the user
    /// message is appended to stored history at REQUEST time. `staged = true`
    /// (P1.2b eager/speculative turns): stored history is NOT mutated — the
    /// user+assistant pair lands only on confirmation via [`Self::commit_turn`],
    /// so a cancelled speculative turn (user kept talking) leaves no
    /// partial-utterance orphan polluting the conversation.
    async fn prepare_messages(
        &self,
        session_id: &str,
        input: Option<&str>,
        staged: bool,
    ) -> Vec<ChatMessage> {
        let mut histories = self.histories.write().await;
        let history = histories
            .entry(session_id.to_string())
            .or_insert_with(|| ConversationHistory::new(self.config.max_history));

        // CONTINUE mode (B-G4 re-inference after tool results): the request
        // is the stored history exactly as-is — no new user message. An
        // EMPTY history would render an empty messages array (provider
        // 400): inject the configured system prompt so the request is at
        // least well-formed (review wf_6783a4b3, unverified #25).
        let Some(input) = input else {
            if history.messages().is_empty()
                && let Some(system_prompt) = &self.config.system_prompt
            {
                history.add(ChatMessage::system(system_prompt));
            }
            return history.messages().to_vec();
        };

        if staged {
            let mut messages = history.messages().to_vec();
            if messages.is_empty()
                && let Some(system_prompt) = &self.config.system_prompt
            {
                messages.push(ChatMessage::system(system_prompt));
            }
            messages.push(ChatMessage::user(input));
            return messages;
        }

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

    /// Commit a CONFIRMED staged turn: system prompt (if first), user message,
    /// and assistant reply land in stored history together, atomically.
    pub async fn commit_turn(&self, session_id: &str, input: &str, assistant_content: &str) {
        let mut histories = self.histories.write().await;
        let history = histories
            .entry(session_id.to_string())
            .or_insert_with(|| ConversationHistory::new(self.config.max_history));
        if history.messages().is_empty()
            && let Some(system_prompt) = &self.config.system_prompt
        {
            history.add(ChatMessage::system(system_prompt));
        }
        history.add(ChatMessage::user(input));
        history.add(ChatMessage::assistant(assistant_content));
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
            self.complete_streaming(session_id, Some(input), api_key, cancel, on_token, false)
                .await
        } else {
            self.complete_sync(session_id, Some(input), api_key, cancel, false).await
        }
    }

    /// Stateless ONE-SHOT completion over an explicit message list — no
    /// session history is read or written (B-G6 summarization, detached
    /// utility inferences). Non-streaming.
    pub async fn complete_detached(
        &self,
        messages: &[ChatMessage],
        api_key: Option<&str>,
        cancel: &CancellationToken,
    ) -> LlmResult<LlmResponse> {
        let resolved = self.resolve_api_key(api_key);
        let api_key = resolved.as_deref();
        let http_request = self.build_http_request(messages, false, api_key);
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
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| self.endpoint_err(format!("Failed to parse response: {}", e)))?;
        let parsed = self.adapter.parse_response(body).map_err(|e| self.endpoint_err(e))?;
        Ok(LlmResponse {
            id: parsed.id,
            model: parsed.model,
            content: parsed.message.content.clone().unwrap_or_default(),
            finish_reason: parsed.finish_reason,
            tool_calls: parsed.message.tool_calls.clone().unwrap_or_default(),
            usage: {
                if let Some(u) = &parsed.usage {
                    export_usage_metrics(self.adapter.kind().as_str(), u);
                }
                parsed.usage
            },
        })
    }

    /// Re-run inference from the stored history AS-IS — no new user message
    /// (B-G4: the follow-up inference after tool results landed; also the
    /// async-tool final delivery turn). The assistant reply is recorded into
    /// history like any turn.
    pub async fn continue_from_history(
        &self,
        session_id: &str,
        api_key: Option<&str>,
        cancel: &CancellationToken,
        on_token: Option<TokenCallback>,
    ) -> LlmResult<LlmResponse> {
        let resolved = self.resolve_api_key(api_key);
        let api_key = resolved.as_deref();
        if self.config.streaming {
            self.complete_streaming(session_id, None, api_key, cancel, on_token, false)
                .await
        } else {
            self.complete_sync(session_id, None, api_key, cancel, false).await
        }
    }

    /// STAGED completion (P1.2b eager turns): identical to [`Self::complete`]
    /// but stored history is untouched — confirm with [`Self::commit_turn`],
    /// or simply drop the result to cancel with zero history pollution.
    pub async fn complete_staged(
        &self,
        session_id: &str,
        input: &str,
        api_key: Option<&str>,
        cancel: &CancellationToken,
        on_token: Option<TokenCallback>,
    ) -> LlmResult<LlmResponse> {
        let resolved = self.resolve_api_key(api_key);
        let api_key = resolved.as_deref();
        if self.config.streaming {
            self.complete_streaming(session_id, Some(input), api_key, cancel, on_token, true)
                .await
        } else {
            self.complete_sync(session_id, Some(input), api_key, cancel, true).await
        }
    }

    #[instrument(skip(self, cancel), fields(model = %self.config.model))]
    async fn complete_sync(
        &self,
        session_id: &str,
        input: Option<&str>,
        api_key: Option<&str>,
        cancel: &CancellationToken,
        staged: bool,
    ) -> LlmResult<LlmResponse> {
        let messages = self.prepare_messages(session_id, input, staged).await;
        let http_request = self.build_http_request(&messages, false, api_key);

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

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| self.endpoint_err(format!("Failed to parse response: {}", e)))?;
        let parsed = self
            .adapter
            .parse_response(body)
            .map_err(|e| self.endpoint_err(e))?;

        if !staged {
            self.record_assistant(session_id, parsed.message.clone()).await;
        }

        Ok(LlmResponse {
            id: parsed.id,
            model: parsed.model,
            content: parsed.message.content.clone().unwrap_or_default(),
            finish_reason: parsed.finish_reason,
            tool_calls: parsed.message.tool_calls.clone().unwrap_or_default(),
            usage: {
                if let Some(u) = &parsed.usage {
                    export_usage_metrics(self.adapter.kind().as_str(), u);
                }
                parsed.usage
            },
        })
    }

    #[instrument(skip(self, cancel, on_token), fields(model = %self.config.model))]
    async fn complete_streaming(
        &self,
        session_id: &str,
        input: Option<&str>,
        api_key: Option<&str>,
        cancel: &CancellationToken,
        on_token: Option<TokenCallback>,
        staged: bool,
    ) -> LlmResult<LlmResponse> {
        let messages = self.prepare_messages(session_id, input, staged).await;
        let http_request = self.build_http_request(&messages, true, api_key);

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
        let mut usage: Option<Usage> = None;
        // index → (id, name, accumulated args)
        let mut tool_call_builders: HashMap<u32, (String, String, String)> = HashMap::new();
        let mut parse_state = adapter::StreamParseState::default();
        let mut stream_done = false;

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

                if line.is_empty() {
                    continue;
                }

                // Vendor-specific framing is normalized by the adapter; this
                // loop only accumulates the normalized events.
                for event in self.adapter.parse_stream_line(&line, &mut parse_state) {
                    match event {
                        LlmStreamEvent::Meta { id, model } => {
                            if completion_id.is_empty()
                                && let Some(id) = id
                            {
                                completion_id = id;
                            }
                            if model_name.is_empty()
                                && let Some(model) = model
                            {
                                model_name = model;
                            }
                        }
                        LlmStreamEvent::TextDelta(content) => {
                            full_content.push_str(&content);
                            if let Some(cb) = &on_token {
                                cb(&content);
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
                        LlmStreamEvent::ToolCallDelta { index, id, name, args_delta } => {
                            let entry = tool_call_builders
                                .entry(index)
                                .or_insert_with(|| (String::new(), String::new(), String::new()));
                            if let Some(id) = id {
                                entry.0 = id;
                            }
                            if let Some(name) = name {
                                entry.1 = name;
                            }
                            if let Some(args) = args_delta {
                                entry.2.push_str(&args);
                            }
                        }
                        LlmStreamEvent::Finish(reason) => {
                            finish_reason = reason;
                        }
                        LlmStreamEvent::Usage(u) => {
                            usage = Some(u);
                        }
                        LlmStreamEvent::StreamEnd => {
                            // Explicit end marker (`data: [DONE]`,
                            // `message_stop`): stop reading the body NOW.
                            // Waiting for the server/proxy to close the
                            // connection (keep-alive linger) would hold the
                            // final sentence — often the whole reply — out of
                            // TTS for the duration (brutal-review finding,
                            // wf_0cc69d62).
                            stream_done = true;
                        }
                        LlmStreamEvent::Error(detail) => {
                            // A vendor-signalled mid-stream failure: surface
                            // it instead of returning the truncated text as
                            // success.
                            return Err(self
                                .endpoint_err(format!("mid-stream provider error: {detail}")));
                        }
                    }
                }
                if stream_done {
                    break;
                }
            }
            if stream_done {
                break;
            }
        }

        let mut tool_calls: Vec<ToolCall> = Vec::new();
        for (_, (id, name, arguments)) in tool_call_builders {
            if !id.is_empty() {
                tool_calls.push(ToolCall {
                    id,
                    call_type: "function".to_string(),
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
        if !staged {
            self.record_assistant(session_id, assistant_message).await;
        }

        Ok(LlmResponse {
            id: completion_id,
            model: model_name,
            content: full_content,
            finish_reason,
            tool_calls,
            // Vendors that report usage while streaming (Anthropic
            // message_delta, Gemini usageMetadata, OpenAI w/ include_usage)
            // now surface it instead of dropping it.
            usage,
        })
    }

    /// Commit a PARTIAL assistant reply cut off by a barge-in (B-G5,
    /// Pipecat parity): the model's next turn must know it was interrupted
    /// mid-answer — without this it repeats or contradicts itself. The
    /// caller guarantees the turn's user message is already in history
    /// (non-staged turns append it at request time), so ordering stays
    /// user → assistant(partial).
    pub async fn commit_partial_assistant(&self, session_id: &str, partial: &str) {
        let partial = partial.trim();
        if partial.is_empty() {
            return;
        }
        let mut histories = self.histories.write().await;
        if let Some(history) = histories.get_mut(session_id) {
            history.add(ChatMessage::assistant(partial));
        }
    }

    /// Add a tool result message to a session's history.
    pub async fn add_tool_result(&self, session_id: &str, tool_call_id: &str, result: &str) {
        let mut histories = self.histories.write().await;
        if let Some(history) = histories.get_mut(session_id) {
            history.add(ChatMessage::tool(tool_call_id, result));
        }
    }

    /// Add a BATCH of tool results under one lock acquisition: a concurrent
    /// turn's user message must never interleave between a batch's results
    /// (pairing violation → strict providers 400 forever).
    pub async fn add_tool_results_batch(
        &self,
        session_id: &str,
        results: Vec<(String, String)>,
    ) {
        let mut histories = self.histories.write().await;
        if let Some(history) = histories.get_mut(session_id) {
            for (id, result) in results {
                history.add(ChatMessage::tool(id, result));
            }
        }
    }

    /// Append messages to a session's context (B-G3 flows: APPEND strategy /
    /// task messages). Creates the session entry if missing.
    pub async fn append_context(&self, session_id: &str, messages: Vec<ChatMessage>) {
        let mut histories = self.histories.write().await;
        let history = histories
            .entry(session_id.to_string())
            .or_insert_with(|| ConversationHistory::new(self.config.max_history));
        for m in messages {
            history.add(m);
        }
    }

    /// REPLACE a session's context wholesale (B-G3 flows: RESET strategy).
    pub async fn set_context(&self, session_id: &str, messages: Vec<ChatMessage>) {
        let mut histories = self.histories.write().await;
        let history = histories
            .entry(session_id.to_string())
            .or_insert_with(|| ConversationHistory::new(self.config.max_history));
        history.clear();
        for m in messages {
            history.add(m);
        }
    }

    /// Set/replace the session's SYSTEM instruction in place (B-G3
    /// `role_message`): replaces the leading system message, or inserts one
    /// at the front. The canonical context stays vendor-neutral — each
    /// adapter renders system placement its own way.
    pub async fn upsert_system(&self, session_id: &str, content: &str) {
        let mut histories = self.histories.write().await;
        let history = histories
            .entry(session_id.to_string())
            .or_insert_with(|| ConversationHistory::new(self.config.max_history));
        history.upsert_system(content);
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

    /// Snapshot a session's history WITH its mutation version (B-G6 CAS).
    /// `None` when the session has no history entry yet.
    pub async fn history_snapshot_versioned(
        &self,
        session_id: &str,
    ) -> Option<(Vec<ChatMessage>, u64)> {
        let histories = self.histories.read().await;
        histories
            .get(session_id)
            .map(|h| (h.messages().to_vec(), h.version()))
    }

    /// Atomically replace a session's context ONLY IF its version is still
    /// `expected_version` (B-G6 lost-update guard, review wf_d43814c3 #2):
    /// a turn that appended during the summary's inference window bumped the
    /// version, so the stale rewrite is refused rather than destroying the
    /// concurrent append (and orphaning tool messages). Returns whether the
    /// replace was applied.
    pub async fn replace_context_if_unchanged(
        &self,
        session_id: &str,
        expected_version: u64,
        messages: Vec<ChatMessage>,
    ) -> bool {
        let mut histories = self.histories.write().await;
        match histories.get_mut(session_id) {
            Some(history) if history.version() == expected_version => {
                history.clear();
                for m in messages {
                    history.add(m);
                }
                true
            }
            _ => false,
        }
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

    /// B-G1: a mid-session vendor switch shares the SAME histories (the
    /// canonical context is vendor-neutral) while swapping the wire format.
    #[tokio::test]
    async fn with_adapter_shares_histories_and_swaps_wire_format() {
        let client = LlmClient::new(LlmClientConfig::default());
        assert_eq!(client.adapter_kind(), AdapterKind::OpenAi);

        // Seed history through the OpenAI-shaped client.
        client
            .commit_turn("s1", "hello", "hi there")
            .await;

        let claude = client.with_adapter(AdapterKind::Anthropic);
        assert_eq!(claude.adapter_kind(), AdapterKind::Anthropic);

        // Same underlying history map: the switched client sees the turn.
        let histories = claude.histories.read().await;
        let h = histories.get("s1").expect("shared history must survive the switch");
        assert_eq!(h.messages().len(), 2);
        drop(histories);

        // And the original client sees writes made through the new one.
        claude.commit_turn("s1", "more", "ok").await;
        let histories = client.histories.read().await;
        assert_eq!(histories.get("s1").unwrap().messages().len(), 4);
    }

    #[test]
    fn provider_kind_selects_adapter() {
        let mut cfg = LlmClientConfig::default();
        cfg.provider_kind = Some(AdapterKind::Gemini);
        assert_eq!(LlmClient::new(cfg).adapter_kind(), AdapterKind::Gemini);
    }

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
