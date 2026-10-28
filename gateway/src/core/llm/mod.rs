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
use std::collections::{BTreeMap, HashMap};
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
    AsyncToolResult, CANCEL_ASYNC_TOOL_NAME, FunctionCallParams, FunctionHandler, FunctionRegistry,
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

/// The request timeout of a config that names none: `WAAV_LLM_TIMEOUT_MS`, else 60 s. DAG LLM nodes
/// default to it too.
pub(crate) fn default_timeout_ms() -> u64 {
    env_ms("WAAV_LLM_TIMEOUT_MS").unwrap_or(60_000)
}

/// A positive whole number of milliseconds from the environment. A malformed value is ignored
/// with a warning, never a failed session.
fn env_ms(name: &str) -> Option<u64> {
    let raw = std::env::var(name).ok()?;
    let v = raw.trim();
    if v.is_empty() {
        return None;
    }
    match v.parse::<u64>() {
        Ok(n) if n > 0 => Some(n),
        _ => {
            warn!(%name, value = %v, "ignored: not a positive whole number of milliseconds");
            None
        }
    }
}

/// The request timeout of a session that asks for its own (`conversation_config.llm_timeout_ms`):
/// at least 1 s, at most `WAAV_LLM_MAX_TIMEOUT_MS` (default 300 s). `None` is the default.
pub fn session_timeout_ms(asked: Option<u64>) -> u64 {
    let max = env_ms("WAAV_LLM_MAX_TIMEOUT_MS")
        .unwrap_or(300_000)
        .max(1_000);
    asked
        .map(|ms| ms.clamp(1_000, max))
        .unwrap_or_else(default_timeout_ms)
}

/// `WAAV_LLM_STREAM_IDLE_TIMEOUT_MS`: when set, a streamed reply is bounded by `timeout_ms` to its
/// first chunk and by this between chunks, instead of by `timeout_ms` as a whole. Unset (default),
/// `timeout_ms` bounds the whole streamed reply, as before.
fn stream_idle_timeout() -> Option<Duration> {
    static IDLE: std::sync::OnceLock<Option<Duration>> = std::sync::OnceLock::new();
    *IDLE.get_or_init(|| env_ms("WAAV_LLM_STREAM_IDLE_TIMEOUT_MS").map(Duration::from_millis))
}

/// The conversation history a session keeps when neither the client nor its alias sets
/// `max_history`: 20 messages, the system prompt included.
pub const DEFAULT_MAX_HISTORY: usize = 20;

fn default_max_history() -> usize {
    DEFAULT_MAX_HISTORY
}

/// Trim `messages` to `max_messages` in blocks, so the start of the conversation an LLM is sent
/// stays the same for many turns and a server's prefix cache (llama.cpp's slot cache, vLLM's
/// prefix caching, OpenAI's prompt caching) reuses all of it but the newest turn.
///
/// Nothing is removed until more than `max_messages` are held. Then, at once, the oldest
/// non-system messages go until half of the non-system room is left (`ceil((max - systems) / 2)`),
/// and on until the oldest one kept opens a turn (a user message), so the window never starts on
/// an assistant reply or an orphaned tool result. System messages are never removed, and neither
/// is the current turn: the last user message and everything after it (its tool rounds and
/// reply), even when that alone is over `max_messages` (a request must carry its user message).
/// With no user message at all, the newest message is kept and the window starts on a message
/// that is not a tool result.
///
/// A history rebuilt by adding a growing list one message at a time (`set_context`) is cut at the
/// same places each time, so a rebuild keeps its prefix too.
pub fn trim_history(messages: &mut Vec<ChatMessage>, max_messages: usize) {
    if messages.len() <= max_messages {
        return;
    }
    let (current, has_user) = match messages.iter().rposition(|m| m.role == MessageRole::User) {
        Some(at) => (at, true),
        // The newest message, with the call its tool results answer.
        None => {
            let mut at = messages.len() - 1;
            while at > 0 && messages[at].role == MessageRole::Tool {
                at -= 1;
            }
            (at, false)
        }
    };
    let systems = messages
        .iter()
        .filter(|m| m.role == MessageRole::System)
        .count();
    let floor = max_messages.saturating_sub(systems).div_ceil(2);
    let opens_turn = |m: &ChatMessage| {
        if has_user {
            m.role == MessageRole::User
        } else {
            m.role != MessageRole::Tool
        }
    };
    let mut others = messages.len() - systems;
    let mut drop = vec![false; messages.len()];
    for (at, m) in messages[..current].iter().enumerate() {
        if m.role == MessageRole::System {
            continue;
        }
        if others <= floor && opens_turn(m) {
            break;
        }
        drop[at] = true;
        others -= 1;
    }
    let mut at = 0;
    messages.retain(|_| {
        at += 1;
        !drop[at - 1]
    });
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
///
/// `Debug` is hand-written and redacts `api_key` and every `headers` value.
#[derive(Clone, Serialize, Deserialize)]
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
    /// OpenAI `parallel_tool_calls`: `false` forces sequential single-tool turns.
    /// `None` omits it (vendor default = parallel allowed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    /// Response format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    /// Stop sequences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    /// Deterministic sampling seed (OpenAI `seed`). Model-agnostic — emitted for
    /// reasoning models too.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// Presence penalty \[-2, 2]. Sampling-family — SUPPRESSED for reasoning
    /// models (they 400 on it), like `temperature`/`top_p`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    /// Frequency penalty \[-2, 2]. Sampling-family — suppressed for reasoning models.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    /// `logprobs` request (bool or `{logprobs, top_logprobs}` shape). Sampling-family
    /// — suppressed for reasoning models.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<serde_json::Value>,
    /// Stable end-user identifier for OpenAI abuse monitoring (`user`). Model-agnostic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Request timeout in milliseconds.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Maximum conversation history to maintain, trimmed in blocks ([`trim_history`]).
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
    /// Whether a missing key may fall back to the process environment. Never (de)serialised: a
    /// client-supplied config must not be able to turn it on. Bud mode forces it off regardless
    /// (FRD-023 RT0).
    #[serde(skip, default = "default_allow_env_fallback")]
    pub allow_env_fallback: bool,
    /// An operator credential this config may use (`core::credentials`), bound to `base_url`'s
    /// host when the session was configured. Read per call. Never (de)serialised.
    #[serde(skip)]
    pub bound_credential: Option<crate::core::credentials::BoundCredential>,
    /// `host:port` entries of `bound_credential` that may be private (port-scoped SSRF grant).
    #[serde(skip)]
    pub private_grants: Vec<crate::core::net::HostPort>,
    /// `base_url` is CLIENT-chosen: build the HTTP client with the connect-time SSRF resolver
    /// (`core::net::guarded_client_builder`). Operator-addressed configs (the Bud gateway, an
    /// operator template) leave it off. Never (de)serialised.
    #[serde(skip)]
    pub guarded_egress: bool,
}

/// The key-resolution rule, over an injected environment lookup so it can be tested without
/// touching the process environment. `env_allowed == false` means the lookup is never called.
pub(crate) fn resolve_llm_api_key(
    per_call: Option<&str>,
    bound: Option<&crate::core::credentials::BoundCredential>,
    config_key: Option<&str>,
    env_allowed: bool,
    default_env_key: &str,
    env: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    if let Some(key) = per_call {
        return Some(key.to_string());
    }
    // An operator credential bound to this target when the session was configured: the only
    // way a CLIENT config reaches an operator secret.
    if let Some(credential) = bound {
        return credential.current();
    }
    if let Some(key) = config_key {
        if key.starts_with("${") && key.ends_with('}') {
            if !env_allowed {
                warn!(
                    "Refused a ${{VAR}} API-key reference: vendor keys never come from the environment in Bud mode"
                );
                return None;
            }
            let var_name = &key[2..key.len() - 1];
            if !ALLOWED_ENV_VARS.contains(&var_name) {
                warn!(
                    var_name = %var_name,
                    "Blocked access to non-whitelisted environment variable"
                );
                return None;
            }
            return env(var_name);
        }
        return Some(key.to_string());
    }
    if !env_allowed {
        return None;
    }
    env(default_env_key)
}

fn default_allow_env_fallback() -> bool {
    true
}

/// The HTTP client for a config: a client-chosen target (`guarded_egress`) also gets the
/// connect-time SSRF resolver (DNS rebinding; names unresolvable at validation time); an
/// operator-addressed one keeps the redirect-checked client.
fn build_http_client(config: &LlmClientConfig) -> (Option<reqwest::Client>, Option<String>) {
    let builder = if config.guarded_egress {
        crate::core::net::guarded_client_builder(
            crate::core::net::HTTP_URL_SCHEMES,
            &config.private_grants,
        )
    } else {
        crate::core::net::ssrf_protected_client_builder(crate::core::net::HTTP_URL_SCHEMES)
    };
    match builder
        // The whole-request timeout is set per request (`build_http_request`), so a
        // streamed reply can be bounded by first chunk and idle gap instead.
        .connect_timeout(Duration::from_millis(
            env_ms("WAAV_LLM_CONNECT_TIMEOUT_MS").unwrap_or(config.timeout_ms),
        ))
        .pool_max_idle_per_host(10)
        .build()
    {
        Ok(client) => (Some(client), None),
        Err(e) => {
            let error = format!("failed to build SSRF-protected LLM HTTP client: {e}");
            tracing::error!(%error);
            (None, Some(error))
        }
    }
}

/// Whether two URLs share scheme, host and port (a key may travel between them).
fn same_origin(a: &str, b: &str) -> bool {
    match (url::Url::parse(a), url::Url::parse(b)) {
        (Ok(a), Ok(b)) => a.origin() == b.origin(),
        _ => false,
    }
}

impl std::fmt::Debug for LlmClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::observability::trace_redact::redacted_debug("LlmClientConfig", self, f)
    }
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
            parallel_tool_calls: None,
            response_format: None,
            stop: None,
            seed: None,
            presence_penalty: None,
            frequency_penalty: None,
            logprobs: None,
            user: None,
            timeout_ms: default_timeout_ms(),
            max_history: default_max_history(),
            headers: HashMap::new(),
            assistant_id: None,
            thread_id: None,
            extra: HashMap::new(),
            provider_kind: None,
            reasoning_effort: None,
            allow_env_fallback: true,
            bound_credential: None,
            private_grants: Vec::new(),
            guarded_egress: false,
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
        // In blocks, from a turn's start: never an orphaned tool result (review wf_6783a4b3,
        // strict providers 400 on one) and a prefix that holds between trims.
        trim_history(&mut self.messages, self.max_messages);
    }

    /// Insert a context message WITHOUT breaking an open `tool_use` pairing.
    /// If the tail is an `assistant{tool_calls}` still awaiting its tool results
    /// (e.g. an async-tool note landing mid-tool-loop), insert BEFORE that
    /// assistant message so the imminent `tool_result` still immediately follows
    /// its `tool_use` — strict providers (Anthropic) 400 otherwise. Else append.
    pub fn add_pairing_safe(&mut self, message: ChatMessage) {
        let tail_is_open_tool_use = self.messages.last().is_some_and(|m| {
            m.role == MessageRole::Assistant
                && m.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty())
        });
        if tail_is_open_tool_use {
            self.version = self.version.wrapping_add(1);
            let idx = self.messages.len() - 1;
            self.messages.insert(idx, message);
        } else {
            self.add(message);
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
    client: Option<reqwest::Client>,
    http_client_init_error: Option<String>,
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
        let (client, http_client_init_error) = build_http_client(&config);
        let adapter = adapter::select_adapter(&config);
        Self {
            config,
            client,
            http_client_init_error,
            adapter,
            histories: Arc::new(RwLock::new(HashMap::new())),
            functions: None,
        }
    }

    /// Re-key this client for a CLIENT-chosen target (`core::credentials::ClientKey`): its
    /// literal key or bound operator credential, no environment fallback, and an HTTP client
    /// with the connect-time SSRF resolver honouring the key's private grants. Shares the
    /// histories (a tier of the same conversation).
    pub fn with_client_key(&self, key: crate::core::credentials::ClientKey) -> Self {
        let mut config = self.config.clone();
        config.api_key = key.literal;
        config.bound_credential = key.bound;
        config.private_grants = key.private_grants;
        config.allow_env_fallback = false;
        config.guarded_egress = true;
        let (client, http_client_init_error) = build_http_client(&config);
        Self {
            adapter: adapter::select_adapter(&config),
            config,
            client,
            http_client_init_error,
            histories: Arc::clone(&self.histories),
            functions: self.functions.clone(),
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
            http_client_init_error: self.http_client_init_error.clone(),
            histories: Arc::clone(&self.histories),
            functions: self.functions.clone(),
        }
    }

    /// S1/S2 (docs/research/REALTIME_REASONING.md §5): clone this client as a second TIER —
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
        max_tokens_override: Option<u32>,
    ) -> Self {
        let mut config = self.config.clone();
        config.model = model;
        let base_url_overridden = base_url.is_some();
        if let Some(b) = base_url {
            // The fast tier's key (literal or bound) belongs to the fast tier's ORIGIN: a
            // reasoning tier on another host must not inherit it — it used to, sending the fast
            // key to a client-chosen `reasoning_base_url`.
            if !same_origin(&config.base_url, &b) {
                config.api_key = None;
                config.bound_credential = None;
                config.private_grants = Vec::new();
            }
            config.base_url = b;
        }
        if let Some(k) = api_key {
            config.api_key = Some(k);
        }
        // A10a: pick the reasoning tier's wire vendor correctly. An EXPLICIT
        // reasoning_provider_kind wins. Otherwise, if the reasoning tier got its
        // OWN base_url (a different host/vendor), DO NOT inherit the fast tier's
        // explicit provider_kind — that would POST e.g. an OpenAI body to an
        // Anthropic host (hard 400 on every escalated turn). Clear it so
        // select_adapter re-infers from the reasoning base_url + model. Only when
        // the base_url is shared do we inherit the fast tier's vendor.
        config.provider_kind = match provider_kind {
            Some(k) => Some(k),
            None if base_url_overridden => None,
            None => config.provider_kind,
        };
        config.reasoning_effort = reasoning_effort;
        // The reasoning tier's output budget is set EXPLICITLY by the caller
        // (build_reasoning_tier computes a think-capable value bounded by the P2
        // ceiling) — it must NOT inherit the fast tier's voice max_tokens cap,
        // which would suppress Anthropic's thinking block entirely.
        if let Some(mt) = max_tokens_override {
            config.max_tokens = Some(mt);
        }
        let kind = adapter::select_adapter(&config).kind();
        Self {
            adapter: adapter::adapter_for(kind),
            config,
            client: self.client.clone(),
            http_client_init_error: self.http_client_init_error.clone(),
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
    ///
    /// FRD-023 RT0 (X-1): in Bud mode the process environment is NEVER read — neither a
    /// `${VAR}` reference nor the default env var. A client-chosen `base_url` with the key
    /// omitted used to send the platform's `OPENAI_API_KEY` to that host.
    pub fn resolve_api_key(&self, per_call: Option<&str>) -> Option<String> {
        let env_allowed =
            self.config.allow_env_fallback && !crate::auth::bud_mode::process_in_bud_mode();
        resolve_llm_api_key(
            per_call,
            self.config.bound_credential.as_ref(),
            self.config.api_key.as_deref(),
            env_allowed,
            self.adapter.default_env_key(),
            |name| std::env::var(name).ok(),
        )
    }

    /// Render a vendor request via the adapter and apply the operator's extra
    /// config headers (vendor auth/version headers come from the adapter).
    fn build_http_request(
        &self,
        messages: &[ChatMessage],
        stream: bool,
        api_key: Option<&str>,
    ) -> LlmResult<reqwest::RequestBuilder> {
        let Some(client) = &self.client else {
            return Err(self.endpoint_err(
                self.http_client_init_error
                    .clone()
                    .unwrap_or_else(|| "LLM HTTP client was not initialized".to_string()),
            ));
        };

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
        let mut req = client
            .post(&rendered.url)
            .header("Content-Type", "application/json");
        // A whole-request timeout also bounds the body: a streamed reply longer than it is cut
        // off mid-sentence. Under a stream-idle timeout `complete_streaming` bounds it instead.
        if !(stream && stream_idle_timeout().is_some()) {
            req = req.timeout(Duration::from_millis(self.config.timeout_ms));
        }
        if stream {
            req = req.header("Accept", "text/event-stream");
        }
        for (k, v) in &rendered.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        for (k, v) in &self.config.headers {
            req = req.header(k, v);
        }
        Ok(req.json(&rendered.body))
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
            && let Some(system_prompt) = &self.config.system_prompt
        {
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
            self.complete_sync(session_id, Some(input), api_key, cancel, false)
                .await
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
        let http_request = self.build_http_request(messages, false, api_key)?;
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
            self.complete_sync(session_id, None, api_key, cancel, false)
                .await
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
            self.complete_sync(session_id, Some(input), api_key, cancel, true)
                .await
        }
    }

    // `api_key` and `input` are never span fields: a span field is printed on EVERY event inside
    // the span (a WARN from the adapter printed the operator's key at the default log level) and
    // is exported to OTLP. `input` is the caller's transcript.
    #[instrument(skip(self, cancel, api_key, input), fields(model = %self.config.model))]
    async fn complete_sync(
        &self,
        session_id: &str,
        input: Option<&str>,
        api_key: Option<&str>,
        cancel: &CancellationToken,
        staged: bool,
    ) -> LlmResult<LlmResponse> {
        let messages = self.prepare_messages(session_id, input, staged).await;
        let http_request = self.build_http_request(&messages, false, api_key)?;

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
            self.record_assistant(session_id, parsed.message.clone())
                .await;
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

    // See `complete_sync`: never record `api_key` / `input` on the span.
    #[instrument(skip(self, cancel, on_token, api_key, input), fields(model = %self.config.model))]
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
        let http_request = self.build_http_request(&messages, true, api_key)?;
        // Under a stream-idle timeout: `timeout_ms` to the first chunk (a model loading on demand,
        // a long prefill), then `idle` between chunks, so a reply that keeps streaming is never
        // cut off. Without one, the request's own whole-request timeout applies.
        let idle = stream_idle_timeout();
        let first_by = tokio::time::Instant::now() + Duration::from_millis(self.config.timeout_ms);
        let first_wait = async {
            match idle {
                Some(_) => tokio::time::sleep_until(first_by).await,
                None => std::future::pending().await,
            }
        };

        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(LlmError::Cancelled),
            _ = first_wait => {
                return Err(self.endpoint_err(format!(
                    "No response within {} ms",
                    self.config.timeout_ms
                )));
            }
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
        // By index, so the calls of a round keep the order the model wrote them in.
        let mut tool_call_builders: BTreeMap<u32, (String, String, String)> = BTreeMap::new();
        let mut parse_state = adapter::StreamParseState::default();
        let mut stream_done = false;

        let mut byte_buffer: Vec<u8> = Vec::new();
        let mut buffer = String::new();
        let mut got_chunk = false;

        loop {
            let wait_until = idle.map(|d| {
                if got_chunk {
                    tokio::time::Instant::now() + d
                } else {
                    first_by
                }
            });
            // Honor cancellation promptly so barge-in/teardown stops token
            // accumulation instead of draining the whole upstream response.
            let chunk_result = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    debug!("LLM stream cancelled");
                    return Err(LlmError::Cancelled);
                }
                _ = async {
                    match wait_until {
                        Some(t) => tokio::time::sleep_until(t).await,
                        None => std::future::pending().await,
                    }
                } => {
                    return Err(self.endpoint_err(match (got_chunk, idle) {
                        (true, Some(d)) => format!("Stream idle for {} ms", d.as_millis()),
                        _ => format!("No first token within {} ms", self.config.timeout_ms),
                    }));
                }
                next = stream.next() => match next {
                    Some(c) => c,
                    None => break,
                }
            };
            got_chunk = true;

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
                        LlmStreamEvent::ToolCallDelta {
                            index,
                            id,
                            name,
                            args_delta,
                        } => {
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
                            return Err(
                                self.endpoint_err(format!("mid-stream provider error: {detail}"))
                            );
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
    pub async fn add_tool_results_batch(&self, session_id: &str, results: Vec<(String, String)>) {
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

    /// S3: append context messages pairing-safely — used for async-tool result
    /// notes, which can land while a (different) turn's tool loop has an open
    /// `assistant{tool_calls}` awaiting results. Inserts before any such open
    /// tool_use so the pairing the tool loop is about to complete stays intact.
    pub async fn append_context_pairing_safe(&self, session_id: &str, messages: Vec<ChatMessage>) {
        let mut histories = self.histories.write().await;
        let history = histories
            .entry(session_id.to_string())
            .or_insert_with(|| ConversationHistory::new(self.config.max_history));
        for m in messages {
            history.add_pairing_safe(m);
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

    /// A session's own LLM timeout is honoured within 1 s .. the server's ceiling (300 s unless
    /// `WAAV_LLM_MAX_TIMEOUT_MS` is set); omitted, the default applies.
    #[test]
    fn a_session_llm_timeout_is_clamped_to_the_server_ceiling() {
        if std::env::var_os("WAAV_LLM_MAX_TIMEOUT_MS").is_some()
            || std::env::var_os("WAAV_LLM_TIMEOUT_MS").is_some()
        {
            return; // the operator's environment decides; nothing to assert
        }
        assert_eq!(session_timeout_ms(None), 60_000);
        assert_eq!(session_timeout_ms(Some(120_000)), 120_000);
        assert_eq!(session_timeout_ms(Some(10)), 1_000);
        assert_eq!(session_timeout_ms(Some(10_000_000)), 300_000);
    }

    /// B-G1: a mid-session vendor switch shares the SAME histories (the
    /// canonical context is vendor-neutral) while swapping the wire format.
    #[tokio::test]
    async fn with_adapter_shares_histories_and_swaps_wire_format() {
        let client = LlmClient::new(LlmClientConfig::default());
        assert_eq!(client.adapter_kind(), AdapterKind::OpenAi);

        // Seed history through the OpenAI-shaped client.
        client.commit_turn("s1", "hello", "hi there").await;

        let claude = client.with_adapter(AdapterKind::Anthropic);
        assert_eq!(claude.adapter_kind(), AdapterKind::Anthropic);

        // Same underlying history map: the switched client sees the turn.
        let histories = claude.histories.read().await;
        let h = histories
            .get("s1")
            .expect("shared history must survive the switch");
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

    #[tokio::test(flavor = "current_thread")]
    async fn llm_client_http_init_failure_surfaces_without_request() {
        let config = LlmClientConfig::default();
        let client = LlmClient {
            adapter: adapter::select_adapter(&config),
            config,
            client: None,
            http_client_init_error: Some("test init failure".to_string()),
            histories: std::sync::Arc::new(RwLock::new(HashMap::new())),
            functions: None,
        };

        let err = client
            .complete_detached(
                &[ChatMessage::user("hello")],
                None,
                &CancellationToken::new(),
            )
            .await
            .expect_err("missing HTTP client must surface as endpoint error");

        match err {
            LlmError::Endpoint { error, .. } => {
                assert!(error.contains("test init failure"), "{error}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn llm_client_redirect_policy_rejects_private_hop() {
        let _env = crate::core::net::ssrf_env_lock();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local redirect test server");
        let addr = listener.local_addr().expect("local addr");

        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
            let response = concat!(
                "HTTP/1.1 302 Found\r\n",
                "Location: http://127.0.0.1:9/metadata\r\n",
                "Content-Length: 0\r\n",
                "\r\n"
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes()).await;
        });

        let client = LlmClient::new(LlmClientConfig {
            base_url: format!("http://{addr}/v1"),
            model: "gpt-test".to_string(),
            ..Default::default()
        });
        let err = client
            .complete_detached(
                &[ChatMessage::user("hello")],
                None,
                &CancellationToken::new(),
            )
            .await
            .expect_err("private redirect target must be rejected");
        match err {
            LlmError::Endpoint { error, .. } => assert!(
                error.contains("error following redirect"),
                "unexpected LLM endpoint error: {error}"
            ),
            other => panic!("unexpected error: {other:?}"),
        }
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
    fn a10a_reasoning_tier_reinfers_vendor_from_its_own_base_url() {
        // Fast tier explicitly OpenAI.
        let fast = LlmClient::new(LlmClientConfig {
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4o-mini".into(),
            provider_kind: Some(AdapterKind::OpenAi),
            ..Default::default()
        });
        // Cross-vendor reasoning tier on the Anthropic host, NO explicit
        // reasoning_provider_kind → must re-infer Anthropic from its OWN host,
        // not inherit OpenAI (which would POST an OpenAI body to api.anthropic.com
        // → 400 every escalated turn).
        let reasoning = fast.with_tier_overrides(
            "claude-3-5-sonnet".into(),
            Some("https://api.anthropic.com/v1".into()),
            None,
            None,
            Some(ReasoningEffort::Low),
            None,
        );
        assert_eq!(
            reasoning.adapter_kind(),
            AdapterKind::Anthropic,
            "cross-vendor reasoning tier must re-infer Anthropic, not inherit OpenAI"
        );
        // No base_url override → still inherit the fast tier's explicit vendor.
        let same_host = fast.with_tier_overrides(
            "o3".into(),
            None,
            None,
            None,
            Some(ReasoningEffort::Low),
            None,
        );
        assert_eq!(
            same_host.adapter_kind(),
            AdapterKind::OpenAi,
            "no base_url override → inherit the fast tier's vendor"
        );
        // Explicit reasoning_provider_kind always wins.
        let explicit = fast.with_tier_overrides(
            "some-model".into(),
            Some("https://proxy.internal/v1".into()),
            None,
            Some(AdapterKind::Anthropic),
            Some(ReasoningEffort::Low),
            None,
        );
        assert_eq!(
            explicit.adapter_kind(),
            AdapterKind::Anthropic,
            "explicit kind wins"
        );
    }

    fn call(id: &str) -> ChatMessage {
        let mut m = ChatMessage::assistant("");
        m.tool_calls = Some(vec![ToolCall {
            id: id.into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "look_up".into(),
                arguments: "{}".into(),
            },
        }]);
        m
    }

    /// Every tool result follows the call it answers, and no message after the system prompt
    /// is a tool result or an assistant reply opening the window.
    fn assert_well_formed(messages: &[ChatMessage]) {
        let mut open: Vec<String> = Vec::new();
        for m in messages {
            if let Some(calls) = &m.tool_calls {
                open.extend(calls.iter().map(|c| c.id.clone()));
            }
            if m.role == MessageRole::Tool {
                let id = m.tool_call_id.clone().unwrap_or_default();
                assert!(
                    open.contains(&id),
                    "orphaned tool result {id}: {messages:?}"
                );
            }
        }
    }

    /// Gaia's report: with one message removed per add, the message after the system prompt
    /// changed every turn once 20 were held, so an LLM server's prefix cache reused only the
    /// system prompt. Trimmed in blocks, the start holds for about five exchanges at a time.
    #[test]
    fn a_full_history_keeps_its_start_between_trims() {
        let mut h = ConversationHistory::new(DEFAULT_MAX_HISTORY);
        h.add(ChatMessage::system("You are a support agent."));
        let mut starts = Vec::new();
        for n in 0..40 {
            h.add(ChatMessage::user(format!("question {n}")));
            let sent = h.messages().to_vec();
            assert!(sent.len() <= DEFAULT_MAX_HISTORY);
            assert_eq!(sent[0].role, MessageRole::System);
            assert_eq!(
                sent[1].role,
                MessageRole::User,
                "the window opens on a turn"
            );
            assert_eq!(
                sent.last().unwrap().content.as_deref(),
                Some(&*format!("question {n}"))
            );
            starts.push(sent[1].content.clone());
            h.add(ChatMessage::assistant(format!("answer {n}")));
        }
        let changes = starts.windows(2).filter(|w| w[0] != w[1]).count();
        // Today's trim changed it on 30 of these 39 turns; in blocks, once per ~5 exchanges.
        assert!(changes <= 8, "{changes} start changes: {starts:?}");
        // Never below half the room: 10 of 19 non-system messages.
        assert!(h.messages().len() > DEFAULT_MAX_HISTORY / 2);
    }

    /// A window smaller than a turn keeps the current turn: the request always carries the
    /// user's message (before, `max_history` 0 or 1 sent the system prompt alone).
    #[test]
    fn a_tiny_window_keeps_the_current_turn() {
        for max in 0..4 {
            let mut h = ConversationHistory::new(max);
            h.add(ChatMessage::system("sys"));
            for n in 0..6 {
                h.add(ChatMessage::user(format!("u{n}")));
                let sent = h.messages();
                assert_eq!(sent[0].role, MessageRole::System, "max {max}");
                assert_eq!(
                    sent.last().unwrap().content.as_deref(),
                    Some(&*format!("u{n}"))
                );
                h.add(ChatMessage::assistant(format!("a{n}")));
                assert!(
                    h.messages()
                        .iter()
                        .any(|m| m.content.as_deref() == Some(&*format!("u{n}")))
                );
            }
        }
    }

    /// A tool round longer than the window stays whole, and older rounds go with their calls.
    #[test]
    fn tool_rounds_are_never_split() {
        let mut h = ConversationHistory::new(6);
        h.add(ChatMessage::system("sys"));
        for n in 0..12 {
            h.add(ChatMessage::user(format!("u{n}")));
            for k in 0..(n % 4) {
                h.add(call(&format!("c{n}-{k}")));
                assert_well_formed(h.messages());
                h.add(ChatMessage::tool(format!("c{n}-{k}"), "result"));
                assert_well_formed(h.messages());
            }
            h.add(ChatMessage::assistant(format!("a{n}")));
            assert_well_formed(h.messages());
            assert_eq!(h.messages()[1].role, MessageRole::User);
        }
    }

    /// System messages added mid-history (async-tool notes, flow task messages) are kept, and
    /// do not starve the conversation: the current turn still goes out.
    #[test]
    fn pinned_system_notes_never_push_out_the_current_turn() {
        let mut h = ConversationHistory::new(4);
        h.add(ChatMessage::system("sys"));
        for n in 0..6 {
            h.add(ChatMessage::user(format!("u{n}")));
            h.add(ChatMessage::system(format!("note {n}")));
            h.add(ChatMessage::assistant(format!("a{n}")));
        }
        h.add(ChatMessage::user("now"));
        let sent = h.messages();
        assert_eq!(sent.last().unwrap().content.as_deref(), Some("now"));
        let systems = sent
            .iter()
            .filter(|m| m.role == MessageRole::System)
            .count();
        assert_eq!(systems, 7, "no system message is removed");
    }

    /// No user message at all (a context rebuilt from assistant and tool rounds): the newest
    /// message and its call are kept, and the window never opens on a tool result.
    #[test]
    fn a_history_without_a_user_message_keeps_its_newest_round() {
        let mut h = ConversationHistory::new(3);
        h.add(ChatMessage::system("sys"));
        for n in 0..5 {
            h.add(ChatMessage::assistant(format!("a{n}")));
            h.add(call(&format!("c{n}")));
            h.add(ChatMessage::tool(format!("c{n}"), "result"));
            assert_well_formed(h.messages());
            assert_ne!(h.messages()[1].role, MessageRole::Tool);
        }
        assert_eq!(h.messages().last().unwrap().role, MessageRole::Tool);
    }

    /// A context rebuilt by adding a growing conversation one message at a time (interaction
    /// runs, flows) is cut at the same places as a history that grew: each rebuild starts where
    /// the last one did, except at a trim.
    #[test]
    fn a_rebuilt_context_starts_where_the_last_rebuild_did() {
        let mut said = vec![ChatMessage::system("sys")];
        let mut grown = ConversationHistory::new(10);
        grown.add(said[0].clone());
        let mut starts = Vec::new();
        for n in 0..30 {
            for m in [
                ChatMessage::user(format!("u{n}")),
                ChatMessage::assistant(format!("a{n}")),
            ] {
                said.push(m.clone());
                grown.add(m);
            }
            let mut rebuilt = ConversationHistory::new(10);
            for m in &said {
                rebuilt.add(m.clone());
            }
            let contents = |h: &ConversationHistory| -> Vec<Option<String>> {
                h.messages().iter().map(|m| m.content.clone()).collect()
            };
            assert_eq!(contents(&rebuilt), contents(&grown));
            starts.push(rebuilt.messages()[1].content.clone());
        }
        let changes = starts.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(changes <= 13, "{changes}: {starts:?}");
    }

    #[test]
    fn add_pairing_safe_inserts_before_open_tool_use() {
        let mut h = ConversationHistory::new(20);
        h.add(ChatMessage::user("book a flight"));
        // An assistant turn with an OPEN tool_use (awaiting its tool result).
        let mut asst = ChatMessage::assistant("");
        asst.tool_calls = Some(vec![ToolCall {
            id: "c1".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "book".into(),
                arguments: "{}".into(),
            },
        }]);
        h.add(asst);

        // An async-tool note lands mid-tool-loop: it must NOT wedge between the
        // tool_use and its imminent tool_result (strict-provider 400).
        h.add_pairing_safe(ChatMessage::system("async note"));
        let msgs = h.messages();
        let last = msgs.last().unwrap();
        assert_eq!(last.role, MessageRole::Assistant);
        assert!(
            last.tool_calls.as_ref().is_some_and(|t| !t.is_empty()),
            "the open tool_use must stay at the tail so its result still pairs"
        );
        assert_eq!(
            msgs[msgs.len() - 2].role,
            MessageRole::System,
            "note inserted before it"
        );

        // Close the pairing with a tool result; now the tail is NOT an open
        // tool_use, so add_pairing_safe appends normally.
        let mut tool = ChatMessage::user("");
        tool.role = MessageRole::Tool;
        tool.tool_call_id = Some("c1".into());
        h.add(tool);
        h.add_pairing_safe(ChatMessage::system("second note"));
        assert_eq!(
            h.messages().last().unwrap().role,
            MessageRole::System,
            "appended at tail"
        );
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

#[cfg(test)]
mod frd023_key_tests {
    //! TC-SEC-02: in Bud mode no path that builds an `LlmClient` reads a key from the environment.
    use super::resolve_llm_api_key;
    use std::cell::RefCell;

    fn canary(reads: &RefCell<Vec<String>>) -> impl Fn(&str) -> Option<String> + '_ {
        move |name| {
            reads.borrow_mut().push(name.to_string());
            Some("sk-canary".to_string())
        }
    }

    #[test]
    fn tc_sec_02_no_env_fallback_when_env_is_not_allowed() {
        let reads = RefCell::new(Vec::new());
        assert_eq!(
            resolve_llm_api_key(None, None, None, false, "OPENAI_API_KEY", canary(&reads)),
            None
        );
        assert_eq!(
            resolve_llm_api_key(
                None,
                None,
                Some("${OPENAI_API_KEY}"),
                false,
                "OPENAI_API_KEY",
                canary(&reads)
            ),
            None
        );
        assert!(
            reads.borrow().is_empty(),
            "the environment was read: {:?}",
            reads.borrow()
        );
    }

    #[test]
    fn tc_sec_02_an_explicit_key_still_wins() {
        let reads = RefCell::new(Vec::new());
        assert_eq!(
            resolve_llm_api_key(
                Some("bud_caller"),
                None,
                None,
                false,
                "OPENAI_API_KEY",
                canary(&reads)
            )
            .as_deref(),
            Some("bud_caller")
        );
        assert_eq!(
            resolve_llm_api_key(
                None,
                None,
                Some("literal"),
                false,
                "OPENAI_API_KEY",
                canary(&reads)
            )
            .as_deref(),
            Some("literal")
        );
        assert!(reads.borrow().is_empty());
    }

    #[test]
    fn standalone_mode_keeps_its_env_fallback() {
        let reads = RefCell::new(Vec::new());
        assert_eq!(
            resolve_llm_api_key(None, None, None, true, "OPENAI_API_KEY", canary(&reads))
                .as_deref(),
            Some("sk-canary")
        );
        assert_eq!(reads.borrow().as_slice(), ["OPENAI_API_KEY".to_string()]);
    }

    #[test]
    fn allow_env_fallback_cannot_be_set_from_a_client_config() {
        let cfg: super::LlmClientConfig = serde_json::from_value(serde_json::json!({
            "base_url": "https://api.openai.com/v1", "model": "m", "allow_env_fallback": false
        }))
        .unwrap();
        assert!(
            cfg.allow_env_fallback,
            "the field is not client-settable in either direction"
        );
        let out = serde_json::to_value(&cfg).unwrap();
        assert!(out.get("allow_env_fallback").is_none());
    }
}
