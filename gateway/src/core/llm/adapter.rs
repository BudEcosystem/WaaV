//! Universal LLM vendor adapters (PIPECAT_FIX_PLAN B-G1).
//!
//! WaaV keeps ONE canonical context shape — the OpenAI-style [`ChatMessage`]
//! list — and translates it **just-in-time, without mutation** to each
//! vendor's wire format. The seam is [`LlmAdapter`]: rendering a request
//! (endpoint + auth headers + body) and parsing both streaming lines and
//! non-streaming bodies back into the same internal types. Everything else in
//! [`super::LlmClient`] (history, staging/eager turns, cancellation,
//! sentence-flush) is vendor-agnostic and untouched.
//!
//! The two genuinely hard transforms (per the Pipecat reference
//! implementation and the fix-plan critique):
//! - **System placement** — OpenAI keeps `system` inline in `messages`;
//!   Anthropic and Gemini take it as a separate top-level field. If the
//!   system message is the *only* message it is converted to a `user`
//!   message instead (both vendors reject an empty conversation).
//! - **Tool-call/result shapes** — OpenAI `{"role":"tool",...}` becomes an
//!   Anthropic `user` message with a `tool_result` block, or a Gemini `user`
//!   `functionResponse` part whose `name` must be resolved from a
//!   `tool_call_id → name` map built while walking earlier messages.
//!
//! Vendor field-name traps encoded here (each 400s the request if wrong):
//! Anthropic REQUIRES top-level `max_tokens`; Gemini wants
//! `generationConfig.maxOutputTokens`; Gemini rejects `additionalProperties`
//! inside tool schemas; Anthropic requires strict user/assistant alternation
//! (consecutive same-role messages are merged) and rejects empty content.

use std::collections::HashMap;

use serde_json::{json, Value};

use super::{
    ChatCompletionChunk, ChatCompletionResponse, ChatMessage, FunctionCall, LlmClientConfig,
    MessageRole, ToolCall, ToolDefinition, Usage,
};

/// Which vendor wire format an adapter speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdapterKind {
    /// OpenAI Chat Completions (and every OpenAI-compatible proxy/provider).
    OpenAi,
    /// Anthropic Messages API (`/v1/messages`).
    Anthropic,
    /// Google Gemini `generateContent` (v1beta).
    Gemini,
}

impl AdapterKind {
    /// Stable label for metrics (`provider` tag — no per-call alloc).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
        }
    }
}

/// D1 (REALTIME_REASONING.md §4.1): the reasoning/thinking-effort dial — ONE
/// typed knob the adapter maps to each vendor's native control, clamped to the
/// model's floor. `Off` *minimizes* thinking; it is not always literally zero,
/// because some 2026 models are adaptive-thinking-only and have a floor — the
/// resolved value is echoed back to the client so the floor is observable.
///
/// Cascade (Chat-Completions) wire forms, AT MOST ONE param per vendor. `Off`/
/// `None` emit NOTHING on every vendor: a non-reasoning model 400s on any
/// thinking param (live-verified against ollama "does not support thinking"),
/// so the safe floor is to send no param at all rather than an explicit zero.
/// - OpenAI: flat string `reasoning_effort` (`Off`→omit);
/// - Anthropic: `thinking { type:"enabled", budget_tokens }` (`Off`→omit;
///   adaptive-only models that reject the param also omit);
/// - Gemini: `generationConfig.thinkingConfig.thinkingBudget` (`Off`→omit) —
///   never paired with `thinkingLevel` (that 400s).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum ReasoningEffort {
    /// Minimize thinking (vendor "none"/budget 0), subject to the model floor.
    Off,
    /// The smallest non-zero effort a vendor exposes.
    Minimal,
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    /// Canonical lowercase label (config echo / metrics — no per-call alloc).
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Total order for floor-clamping (Off < Minimal < Low < Medium < High).
    #[inline]
    const fn rank(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Minimal => 1,
            Self::Low => 2,
            Self::Medium => 3,
            Self::High => 4,
        }
    }

    /// The minimum effort a model can actually honor. Adaptive-thinking-only
    /// families cannot be driven to zero reasoning → floor at `Low` so the echoed
    /// value is honest. Everything else floors at `Off`.
    pub fn floor_for_model(model: &str) -> Self {
        if is_adaptive_reasoning_only(model) { Self::Low } else { Self::Off }
    }

    /// Raise `self` to at least `floor`.
    pub fn clamp_to_floor(self, floor: Self) -> Self {
        if self.rank() >= floor.rank() { self } else { floor }
    }

    /// Single source of truth: returns `(applied, floor)` where `applied` is the
    /// floor-clamped value to actually send (`None` when nothing was requested →
    /// vendor default, no param emitted) and `floor` is always the model's floor
    /// (for the session-ack echo).
    pub fn resolve(model: &str, requested: Option<Self>) -> (Option<Self>, Self) {
        let floor = Self::floor_for_model(model);
        (requested.map(|r| r.clamp_to_floor(floor)), floor)
    }
}

/// Anthropic minimum `budget_tokens` for the `type:"enabled"` thinking form.
/// Below this, the API 400s — so we don't emit the enabled form (see render).
const ANTHROPIC_MIN_THINKING_BUDGET: u32 = 1024;

/// SINGLE source of truth for the adaptive-thinking-ONLY model families (Opus
/// 4.7/4.8, Gemini Fable/Mythos, Fable-5, claude-fable). These cannot be driven
/// to zero reasoning and REJECT an explicit `thinking:{type:"enabled",budget}`
/// (Anthropic) — they only honor their built-in adaptive thinking. Both the D1
/// floor (`floor_for_model`) and the Anthropic render gate
/// (`is_anthropic_adaptive_only`) derive from this list so they can never drift.
pub(crate) fn is_adaptive_reasoning_only(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.contains("opus-4-7")
        || m.contains("opus-4.7")
        || m.contains("opus-4-8")
        || m.contains("opus-4.8")
        || m.contains("fable-5")
        || m.contains("claude-fable")
        || m.contains("gemini-fable")
        || m.contains("gemini-mythos")
        || m.contains("gemini-3-mythos")
}

/// Whether an Anthropic model is adaptive-thinking-ONLY → emit no thinking block.
/// (The Anthropic render only runs for Anthropic models, so the gemini entries in
/// the shared list are inert here.)
fn is_anthropic_adaptive_only(model: &str) -> bool {
    is_adaptive_reasoning_only(model)
}

/// Anthropic extended-thinking budget per effort. The caller MUST further clamp
/// to `< max_tokens` (Anthropic 400s otherwise).
fn anthropic_budget_tokens(e: ReasoningEffort) -> u32 {
    match e {
        ReasoningEffort::Off => 0,
        ReasoningEffort::Minimal => 1024,
        ReasoningEffort::Low => 2048,
        ReasoningEffort::Medium => 4096,
        ReasoningEffort::High => 8192,
    }
}

/// Gemini `thinkingBudget` per effort (`0` disables; cap 24576).
fn gemini_budget(e: ReasoningEffort) -> u32 {
    match e {
        ReasoningEffort::Off => 0,
        ReasoningEffort::Minimal => 1024,
        ReasoningEffort::Low => 4096,
        ReasoningEffort::Medium => 12288,
        ReasoningEffort::High => 24576,
    }
}

/// A fully rendered HTTP request: vendor endpoint + auth headers + JSON body.
#[derive(Debug, Clone)]
pub struct RenderedRequest {
    pub url: String,
    /// Header name/value pairs INCLUDING auth (rendered from the resolved
    /// key) and any vendor-version headers. `Content-Type`/`Accept` are
    /// applied by the caller.
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

/// One normalized streaming event, vendor-agnostic.
#[derive(Debug, Clone)]
pub enum LlmStreamEvent {
    /// Completion/message identifiers (first occurrence wins at the client).
    Meta { id: Option<String>, model: Option<String> },
    /// A text content delta (feeds the token callback / sentence aggregator).
    TextDelta(String),
    /// A tool-call fragment. `index` correlates fragments of the same call;
    /// `id`/`name` arrive once (vendor-dependent), `args_delta` accumulates.
    ToolCallDelta {
        index: u32,
        id: Option<String>,
        name: Option<String>,
        args_delta: Option<String>,
    },
    /// The vendor's finish/stop reason.
    Finish(Option<String>),
    /// Token usage (last occurrence wins).
    Usage(Usage),
    /// The vendor's explicit end-of-stream marker (`data: [DONE]`,
    /// `message_stop`). The client stops reading the body immediately —
    /// waiting for the connection to close would hold the final sentence out
    /// of TTS for the keep-alive linger.
    StreamEnd,
    /// A vendor-signalled MID-STREAM error (Anthropic `error` frames,
    /// OpenAI-compatible `{"error":…}` data lines). Without this, an
    /// `overloaded_error` mid-reply silently truncates the response and
    /// returns the partial text as success.
    Error(String),
}

/// Mutable per-request stream-parse state (vendor-specific bookkeeping).
#[derive(Debug, Default)]
pub struct StreamParseState {
    /// Whether `Meta` was already emitted (it repeats on every OpenAI chunk;
    /// emitting once avoids two String clones + a Vec entry per SSE chunk on
    /// the realtime hot path).
    meta_sent: bool,
    /// Anthropic: the current SSE `event:` type (its SSE frames are
    /// event-typed; the meaning of `data:` depends on the preceding event).
    current_event: Option<String>,
    /// Anthropic: prompt tokens from `message_start`, merged into the final
    /// usage at `message_delta`.
    input_tokens: u32,
    /// Gemini: running index for whole-call `functionCall` parts.
    next_tool_index: u32,
}

/// A parsed non-streaming completion, normalized to the internal types.
#[derive(Debug, Clone)]
pub struct ParsedCompletion {
    pub id: String,
    pub model: String,
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
    pub usage: Option<Usage>,
}

/// The standardized vendor seam. Every LLM provider speaks through exactly
/// this trait — never per-provider branches at call sites.
pub trait LlmAdapter: Send + Sync + std::fmt::Debug {
    fn kind(&self) -> AdapterKind;

    /// The env var consulted when neither a per-call nor a config key is set.
    fn default_env_key(&self) -> &'static str;

    /// Render the canonical message list into a vendor request. PURE — never
    /// mutates the stored context (the same history can be re-rendered for a
    /// different vendor mid-session without poisoning).
    fn render_request(
        &self,
        messages: &[ChatMessage],
        cfg: &LlmClientConfig,
        stream: bool,
        api_key: Option<&str>,
    ) -> RenderedRequest;

    /// Parse one SSE/stream line into 0+ normalized events.
    fn parse_stream_line(&self, line: &str, state: &mut StreamParseState) -> Vec<LlmStreamEvent>;

    /// Parse a non-streaming response body.
    fn parse_response(&self, body: Value) -> Result<ParsedCompletion, String>;
}

/// Select the adapter for a config: explicit `provider_kind` wins; otherwise
/// only the two CANONICAL vendor hosts are inferred (an OpenAI-compatible
/// proxy in front of Claude/Gemini speaks the OpenAI wire format — its URL
/// says nothing about the body shape, so never guess beyond the canonical
/// hosts).
pub fn select_adapter(cfg: &LlmClientConfig) -> &'static dyn LlmAdapter {
    let kind = cfg.provider_kind.unwrap_or_else(|| {
        // EXACT host comparison, not substring: `api.anthropic.com.evil.com`
        // must not select the Anthropic adapter (whose env-key fallback would
        // then send ANTHROPIC_API_KEY to the attacker's host).
        let host = url::Url::parse(&cfg.base_url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        match host.as_str() {
            "api.anthropic.com" => AdapterKind::Anthropic,
            "generativelanguage.googleapis.com" => AdapterKind::Gemini,
            _ => AdapterKind::OpenAi,
        }
    });
    adapter_for(kind)
}

/// The adapter instance for a kind (stateless, so 'static).
pub fn adapter_for(kind: AdapterKind) -> &'static dyn LlmAdapter {
    match kind {
        AdapterKind::OpenAi => &OpenAiAdapter,
        AdapterKind::Anthropic => &AnthropicAdapter,
        AdapterKind::Gemini => &GeminiAdapter,
    }
}

// =============================================================================
// Shared helpers
// =============================================================================

/// Split a leading in-context system message from the rest. If the system
/// message is the ONLY message, it is converted to `user` (Anthropic/Gemini
/// reject an empty conversation) and no system instruction is returned.
fn split_system(messages: &[ChatMessage]) -> (Option<String>, Vec<&ChatMessage>) {
    match messages.split_first() {
        Some((first, rest)) if first.role == MessageRole::System => {
            let sys = first.content.clone().unwrap_or_default();
            if rest.is_empty() {
                // Converted to user below by returning it in the remainder
                // with no system: the caller renders content-only messages.
                (None, vec![first])
            } else {
                (Some(sys), rest.iter().collect())
            }
        }
        _ => (None, messages.iter().collect()),
    }
}

/// Recursively strip `additionalProperties` (Gemini rejects it) from a JSON
/// schema value.
fn strip_additional_properties(v: &Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| k.as_str() != "additionalProperties")
                .map(|(k, val)| (k.clone(), strip_additional_properties(val)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(strip_additional_properties).collect()),
        other => other.clone(),
    }
}

fn parse_args_object(arguments: &str) -> Value {
    serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| {
        // Vendor APIs require an OBJECT for tool input; a malformed/partial
        // argument string is preserved rather than dropped.
        json!({ "_raw": arguments })
    })
}

// =============================================================================
// OpenAI
// =============================================================================

/// OpenAI Chat Completions — a pure refactor of the legacy in-client logic
/// (byte-identical request body, incl. the `max_completion_tokens: 4096`
/// default when `max_tokens` is unset).
#[derive(Debug)]
pub struct OpenAiAdapter;

impl LlmAdapter for OpenAiAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::OpenAi
    }

    fn default_env_key(&self) -> &'static str {
        "OPENAI_API_KEY"
    }

    fn render_request(
        &self,
        messages: &[ChatMessage],
        cfg: &LlmClientConfig,
        stream: bool,
        api_key: Option<&str>,
    ) -> RenderedRequest {
        let mut body = json!({
            "model": cfg.model,
            "messages": messages,
            "stream": stream,
        });
        let obj = body.as_object_mut().expect("object literal");
        if let Some(t) = cfg.temperature {
            obj.insert("temperature".into(), json!(t));
        }
        if let Some(p) = cfg.top_p {
            obj.insert("top_p".into(), json!(p));
        }
        match cfg.max_tokens {
            Some(m) => {
                obj.insert("max_tokens".into(), json!(m));
            }
            None => {
                obj.insert("max_completion_tokens".into(), json!(4096));
            }
        }
        // D1: OpenAI Chat-Completions takes the FLAT string `reasoning_effort`.
        // CRITICAL (live-verified): a NON-reasoning model 400s on ANY thinking
        // param ("model does not support thinking"), and the recommended path is
        // a FAST model — so `Off`/`None` emit NOTHING (no param, never a 400).
        // Only an explicit `Minimal..High` emits the param (use it on a
        // reasoning-capable model); the floor-clamp upgrades adaptive-only models
        // to `Low`, which then emits. EXACTLY ONE param (no nested `reasoning`).
        // Inserted before the `cfg.extra` flatten so an explicit knob wins.
        if let Some(effort) = cfg.reasoning_effort {
            if effort != ReasoningEffort::Off {
                obj.insert("reasoning_effort".into(), json!(effort.as_str()));
            }
        }
        if let Some(tools) = &cfg.tools {
            obj.insert("tools".into(), serde_json::to_value(tools).unwrap_or(Value::Null));
        }
        if let Some(tc) = &cfg.tool_choice {
            obj.insert("tool_choice".into(), tc.clone());
        }
        if let Some(rf) = &cfg.response_format {
            obj.insert("response_format".into(), serde_json::to_value(rf).unwrap_or(Value::Null));
        }
        if let Some(stop) = &cfg.stop {
            obj.insert("stop".into(), json!(stop));
        }
        for (k, v) in &cfg.extra {
            obj.insert(k.clone(), v.clone());
        }

        let mut headers = Vec::new();
        if let Some(key) = api_key {
            headers.push(("Authorization".into(), format!("Bearer {key}")));
        }
        if let Some(org) = &cfg.organization {
            headers.push(("OpenAI-Organization".into(), org.clone()));
        }

        RenderedRequest {
            url: format!("{}/chat/completions", cfg.base_url.trim_end_matches('/')),
            headers,
            body,
        }
    }

    fn parse_stream_line(&self, line: &str, state: &mut StreamParseState) -> Vec<LlmStreamEvent> {
        let Some(data) = line.strip_prefix("data: ") else {
            return Vec::new();
        };
        if data == "[DONE]" {
            return vec![LlmStreamEvent::StreamEnd];
        }
        let Ok(chunk) = serde_json::from_str::<ChatCompletionChunk>(data) else {
            // Some OpenAI-compatible providers stream `{"error":{...}}` as a
            // data line — surface it instead of silently truncating.
            if let Ok(v) = serde_json::from_str::<Value>(data)
                && let Some(err) = v.get("error")
            {
                return vec![LlmStreamEvent::Error(err.to_string())];
            }
            tracing::warn!(data = %data, "failed to parse OpenAI streaming chunk");
            return Vec::new();
        };
        let mut events = Vec::new();
        if !state.meta_sent {
            state.meta_sent = true;
            events.push(LlmStreamEvent::Meta {
                id: Some(chunk.id.clone()),
                model: Some(chunk.model.clone()),
            });
        }
        for choice in &chunk.choices {
            if let Some(content) = &choice.delta.content {
                events.push(LlmStreamEvent::TextDelta(content.clone()));
            }
            if let Some(reason) = &choice.finish_reason {
                events.push(LlmStreamEvent::Finish(Some(reason.clone())));
            }
            if let Some(tcs) = &choice.delta.tool_calls {
                for tc in tcs {
                    events.push(LlmStreamEvent::ToolCallDelta {
                        index: tc.index,
                        id: tc.id.clone(),
                        name: tc.function.as_ref().and_then(|f| f.name.clone()),
                        args_delta: tc.function.as_ref().and_then(|f| f.arguments.clone()),
                    });
                }
            }
        }
        if let Some(usage) = chunk.usage {
            events.push(LlmStreamEvent::Usage(usage));
        }
        events
    }

    fn parse_response(&self, body: Value) -> Result<ParsedCompletion, String> {
        let completion: ChatCompletionResponse =
            serde_json::from_value(body).map_err(|e| format!("OpenAI response parse: {e}"))?;
        let choice = completion.choices.first().ok_or("no choices in response")?;
        Ok(ParsedCompletion {
            id: completion.id,
            model: completion.model,
            message: choice.message.clone(),
            finish_reason: choice.finish_reason.clone(),
            usage: completion.usage,
        })
    }
}

// =============================================================================
// Anthropic
// =============================================================================

const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Anthropic REQUIRES `max_tokens`; this default matches the reference
/// implementation when the operator hasn't set one.
const ANTHROPIC_DEFAULT_MAX_TOKENS: u32 = 4096;

#[derive(Debug)]
pub struct AnthropicAdapter;

impl AnthropicAdapter {
    /// Convert canonical messages to Anthropic's content-block form:
    /// `tool` → `user` + `tool_result` block; assistant `tool_calls` →
    /// `tool_use` blocks (args JSON-parsed to an object); consecutive
    /// same-role messages merged (strict alternation); empty content →
    /// `"(empty)"` (Anthropic rejects empty).
    fn convert_messages(messages: &[&ChatMessage]) -> Vec<Value> {
        let mut out: Vec<(String, Vec<Value>)> = Vec::new();

        for msg in messages {
            // Anthropic requires the FIRST message to be a user turn. History
            // trimming (`ConversationHistory::add` drops the OLDEST non-system
            // message one at a time) can leave an assistant message first once
            // the window wraps — without this guard every subsequent call
            // 400s (review wf_e1bdd231, self-verified).
            if out.is_empty() && msg.role == MessageRole::Assistant {
                out.push((
                    "user".to_string(),
                    vec![json!({ "type": "text", "text": "(history truncated)" })],
                ));
            }
            let (role, blocks): (&str, Vec<Value>) = match msg.role {
                MessageRole::Tool | MessageRole::Function => {
                    let block = json!({
                        "type": "tool_result",
                        "tool_use_id": msg.tool_call_id.clone().unwrap_or_default(),
                        "content": msg.content.clone().unwrap_or_else(|| "(empty)".into()),
                    });
                    ("user", vec![block])
                }
                MessageRole::Assistant => {
                    let mut blocks = Vec::new();
                    if let Some(text) = &msg.content
                        && !text.is_empty()
                    {
                        blocks.push(json!({ "type": "text", "text": text }));
                    }
                    if let Some(calls) = &msg.tool_calls {
                        for call in calls {
                            blocks.push(json!({
                                "type": "tool_use",
                                "id": call.id,
                                "name": call.function.name,
                                "input": parse_args_object(&call.function.arguments),
                            }));
                        }
                    }
                    if blocks.is_empty() {
                        blocks.push(json!({ "type": "text", "text": "(empty)" }));
                    }
                    ("assistant", blocks)
                }
                // System mid-conversation (rare; the head system was already
                // extracted) and User both render as user turns.
                MessageRole::System | MessageRole::User => {
                    let text = match msg.content.as_deref() {
                        Some(t) if !t.is_empty() => t.to_string(),
                        _ => "(empty)".into(),
                    };
                    ("user", vec![json!({ "type": "text", "text": text })])
                }
            };

            match out.last_mut() {
                Some((last_role, last_blocks)) if last_role == role => {
                    last_blocks.extend(blocks);
                }
                _ => out.push((role.to_string(), blocks)),
            }
        }

        out.into_iter()
            .map(|(role, content)| json!({ "role": role, "content": content }))
            .collect()
    }

    fn convert_tools(tools: &[ToolDefinition]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.function.name,
                    "description": t.function.description,
                    "input_schema": t.function.parameters,
                })
            })
            .collect()
    }
}

impl LlmAdapter for AnthropicAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::Anthropic
    }

    fn default_env_key(&self) -> &'static str {
        "ANTHROPIC_API_KEY"
    }

    fn render_request(
        &self,
        messages: &[ChatMessage],
        cfg: &LlmClientConfig,
        stream: bool,
        api_key: Option<&str>,
    ) -> RenderedRequest {
        let (system, rest) = split_system(messages);
        // A lone system message was returned in `rest`: render it as user.
        let converted = if system.is_none() && rest.len() == 1 && rest[0].role == MessageRole::System
        {
            vec![json!({
                "role": "user",
                "content": [{ "type": "text",
                              "text": rest[0].content.clone().unwrap_or_else(|| "(empty)".into()) }],
            })]
        } else {
            Self::convert_messages(&rest)
        };

        let mut body = json!({
            "model": cfg.model,
            "messages": converted,
            // REQUIRED by the Messages API — omitting it is a hard 400.
            "max_tokens": cfg.max_tokens.unwrap_or(ANTHROPIC_DEFAULT_MAX_TOKENS),
            "stream": stream,
        });
        let obj = body.as_object_mut().expect("object literal");
        if let Some(sys) = system {
            obj.insert("system".into(), json!(sys));
        }
        // D1: Anthropic extended thinking (brutal-review hardened). `Off`/`None`
        // → nothing. The `enabled` form requires `1024 <= budget_tokens <
        // max_tokens`; the voice cap (256) makes that jointly unsatisfiable, and
        // adaptive-only models reject the `enabled` form entirely. So:
        // adaptive-only → nothing (model thinks adaptively by default); otherwise
        // emit `enabled` ONLY when a valid budget fits — else nothing rather than
        // a guaranteed 400. Computed FIRST because thinking is INCOMPATIBLE with
        // sampling params (temperature/top_p), which we then suppress.
        let thinking_block: Option<Value> = cfg.reasoning_effort.and_then(|effort| {
            if effort == ReasoningEffort::Off || is_anthropic_adaptive_only(&cfg.model) {
                return None;
            }
            let max_tok = cfg.max_tokens.unwrap_or(ANTHROPIC_DEFAULT_MAX_TOKENS);
            let budget = anthropic_budget_tokens(effort).min(max_tok.saturating_sub(1));
            (budget >= ANTHROPIC_MIN_THINKING_BUDGET)
                .then(|| json!({ "type": "enabled", "budget_tokens": budget }))
        });
        // Sampling params are a hard 400 WITH extended thinking enabled
        // (temperature must be unset/1.0; top_p/top_k unsupported) — so emit them
        // ONLY when no thinking block is sent.
        if thinking_block.is_none() {
            if let Some(t) = cfg.temperature {
                obj.insert("temperature".into(), json!(t));
            }
            if let Some(p) = cfg.top_p {
                obj.insert("top_p".into(), json!(p));
            }
        }
        if let Some(thinking) = thinking_block {
            obj.insert("thinking".into(), thinking);
        }
        if let Some(stop) = &cfg.stop {
            obj.insert("stop_sequences".into(), json!(stop));
        }
        if let Some(tools) = &cfg.tools
            && !tools.is_empty()
        {
            obj.insert("tools".into(), Value::Array(Self::convert_tools(tools)));
        }
        for (k, v) in &cfg.extra {
            obj.insert(k.clone(), v.clone());
        }

        // Base URL handling: canonical "https://api.anthropic.com" →
        // "/v1/messages"; a base already ending in "/v1" → "/messages".
        let base = cfg.base_url.trim_end_matches('/');
        let url = if base.ends_with("/v1") {
            format!("{base}/messages")
        } else {
            format!("{base}/v1/messages")
        };

        let mut headers = vec![("anthropic-version".into(), ANTHROPIC_VERSION.into())];
        if let Some(key) = api_key {
            headers.push(("x-api-key".into(), key.to_string()));
        }

        RenderedRequest { url, headers, body }
    }

    fn parse_stream_line(&self, line: &str, state: &mut StreamParseState) -> Vec<LlmStreamEvent> {
        if let Some(event) = line.strip_prefix("event: ") {
            state.current_event = Some(event.trim().to_string());
            return Vec::new();
        }
        let Some(data) = line.strip_prefix("data: ") else {
            return Vec::new();
        };
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            tracing::warn!(data = %data, "failed to parse Anthropic streaming frame");
            return Vec::new();
        };
        // The data frame also self-describes via "type" — prefer it over the
        // event line (robust to coalesced frames).
        let frame_type = v
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| state.current_event.clone())
            .unwrap_or_default();

        match frame_type.as_str() {
            "message_start" => {
                let msg = v.get("message").cloned().unwrap_or(Value::Null);
                state.input_tokens = msg
                    .pointer("/usage/input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32;
                vec![LlmStreamEvent::Meta {
                    id: msg.get("id").and_then(Value::as_str).map(str::to_string),
                    model: msg.get("model").and_then(Value::as_str).map(str::to_string),
                }]
            }
            "content_block_start" => {
                let index = v.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
                let block = v.get("content_block").cloned().unwrap_or(Value::Null);
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    vec![LlmStreamEvent::ToolCallDelta {
                        index,
                        id: block.get("id").and_then(Value::as_str).map(str::to_string),
                        name: block.get("name").and_then(Value::as_str).map(str::to_string),
                        args_delta: None,
                    }]
                } else {
                    Vec::new()
                }
            }
            "content_block_delta" => {
                let index = v.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
                let delta = v.get("delta").cloned().unwrap_or(Value::Null);
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => delta
                        .get("text")
                        .and_then(Value::as_str)
                        .map(|t| vec![LlmStreamEvent::TextDelta(t.to_string())])
                        .unwrap_or_default(),
                    Some("input_json_delta") => delta
                        .get("partial_json")
                        .and_then(Value::as_str)
                        .map(|j| {
                            vec![LlmStreamEvent::ToolCallDelta {
                                index,
                                id: None,
                                name: None,
                                args_delta: Some(j.to_string()),
                            }]
                        })
                        .unwrap_or_default(),
                    _ => Vec::new(),
                }
            }
            "message_delta" => {
                let mut events = Vec::new();
                if let Some(reason) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    events.push(LlmStreamEvent::Finish(Some(reason.to_string())));
                }
                if let Some(output) = v.pointer("/usage/output_tokens").and_then(Value::as_u64) {
                    let completion = output as u32;
                    events.push(LlmStreamEvent::Usage(Usage {
                        prompt_tokens: state.input_tokens,
                        completion_tokens: completion,
                        total_tokens: state.input_tokens + completion,
                        prompt_tokens_details: None,
                        completion_tokens_details: None,
                    }));
                }
                events
            }
            "message_stop" => vec![LlmStreamEvent::StreamEnd],
            // A mid-stream error (overloaded_error, api_error…) must surface
            // as an error, not a silent truncation of the reply.
            "error" => {
                let detail = v.get("error").map(|e| e.to_string()).unwrap_or_else(|| data.to_string());
                vec![LlmStreamEvent::Error(detail)]
            }
            // "ping", "content_block_stop" are intentionally inert.
            _ => Vec::new(),
        }
    }

    fn parse_response(&self, body: Value) -> Result<ParsedCompletion, String> {
        let id = body.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let model = body.get("model").and_then(Value::as_str).unwrap_or_default().to_string();
        let mut text = String::new();
        let mut tool_calls = Vec::new();
        for block in body.get("content").and_then(Value::as_array).into_iter().flatten() {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(Value::as_str) {
                        text.push_str(t);
                    }
                }
                Some("tool_use") => tool_calls.push(ToolCall {
                    id: block.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                    call_type: "function".into(),
                    function: FunctionCall {
                        name: block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        arguments: block
                            .get("input")
                            .map(|i| i.to_string())
                            .unwrap_or_else(|| "{}".into()),
                    },
                }),
                _ => {}
            }
        }
        let usage = body.get("usage").map(|u| {
            let prompt = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0) as u32;
            let completion = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0) as u32;
            Usage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: prompt + completion,
                prompt_tokens_details: None,
                completion_tokens_details: None,
            }
        });
        let message = ChatMessage {
            role: MessageRole::Assistant,
            content: if text.is_empty() { None } else { Some(text) },
            name: None,
            tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls.clone()) },
            tool_call_id: None,
            function_call: None,
        };
        Ok(ParsedCompletion {
            id,
            model,
            message,
            finish_reason: body
                .get("stop_reason")
                .and_then(Value::as_str)
                .map(str::to_string),
            usage,
        })
    }
}

// =============================================================================
// Gemini
// =============================================================================

const GEMINI_DEFAULT_MAX_TOKENS: u32 = 4096;

#[derive(Debug)]
pub struct GeminiAdapter;

impl GeminiAdapter {
    /// Convert canonical messages to Gemini `contents`: `assistant` → `model`;
    /// `tool` → `user` `functionResponse` part whose `name` is resolved from a
    /// `tool_call_id → name` map (Gemini matches responses by NAME, not id);
    /// non-object results wrapped as `{"value": ...}` (Gemini requires a
    /// JSON object).
    fn convert_contents(messages: &[&ChatMessage]) -> Vec<Value> {
        // First pass: build the id → name map from assistant tool_calls.
        let mut id_to_name: HashMap<&str, &str> = HashMap::new();
        for msg in messages {
            if let Some(calls) = &msg.tool_calls {
                for call in calls {
                    id_to_name.insert(call.id.as_str(), call.function.name.as_str());
                }
            }
        }

        let mut out = Vec::new();
        for msg in messages {
            match msg.role {
                MessageRole::Tool | MessageRole::Function => {
                    let name = msg
                        .tool_call_id
                        .as_deref()
                        .and_then(|id| id_to_name.get(id).copied())
                        .unwrap_or("tool_call_result");
                    let response = match msg
                        .content
                        .as_deref()
                        .and_then(|c| serde_json::from_str::<Value>(c).ok())
                    {
                        Some(Value::Object(o)) => Value::Object(o),
                        Some(other) => json!({ "value": other }),
                        None => json!({ "value": msg.content.clone().unwrap_or_default() }),
                    };
                    out.push(json!({
                        "role": "user",
                        "parts": [{ "functionResponse": { "name": name, "response": response } }],
                    }));
                }
                MessageRole::Assistant => {
                    let mut parts = Vec::new();
                    if let Some(text) = &msg.content
                        && !text.is_empty()
                    {
                        parts.push(json!({ "text": text }));
                    }
                    if let Some(calls) = &msg.tool_calls {
                        for call in calls {
                            parts.push(json!({
                                "functionCall": {
                                    "name": call.function.name,
                                    "args": parse_args_object(&call.function.arguments),
                                }
                            }));
                        }
                    }
                    if parts.is_empty() {
                        // Gemini rejects an empty parts list / empty text.
                        parts.push(json!({ "text": "(empty)" }));
                    }
                    out.push(json!({ "role": "model", "parts": parts }));
                }
                MessageRole::System | MessageRole::User => {
                    out.push(json!({
                        "role": "user",
                        "parts": [{ "text": msg.content.clone().unwrap_or_default() }],
                    }));
                }
            }
        }
        out
    }
}

impl LlmAdapter for GeminiAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::Gemini
    }

    fn default_env_key(&self) -> &'static str {
        "GOOGLE_AI_API_KEY"
    }

    fn render_request(
        &self,
        messages: &[ChatMessage],
        cfg: &LlmClientConfig,
        stream: bool,
        api_key: Option<&str>,
    ) -> RenderedRequest {
        let (system, rest) = split_system(messages);

        let mut body = json!({
            "contents": Self::convert_contents(&rest),
        });
        let obj = body.as_object_mut().expect("object literal");
        if let Some(sys) = system {
            obj.insert("systemInstruction".into(), json!({ "parts": [{ "text": sys }] }));
        }

        let mut generation_config = serde_json::Map::new();
        if let Some(t) = cfg.temperature {
            generation_config.insert("temperature".into(), json!(t));
        }
        if let Some(p) = cfg.top_p {
            generation_config.insert("topP".into(), json!(p));
        }
        generation_config.insert(
            "maxOutputTokens".into(),
            json!(cfg.max_tokens.unwrap_or(GEMINI_DEFAULT_MAX_TOKENS)),
        );
        if let Some(stop) = &cfg.stop {
            generation_config.insert("stopSequences".into(), json!(stop));
        }
        // D1: Gemini thinking via the numeric `thinkingBudget` ONLY — NEVER
        // paired with `thinkingLevel` (Gemini 400s on conflicting thinking
        // params). Like OpenAI, `Off`/`None` emit NOTHING (a non-thinking model
        // rejects a thinkingConfig); only `Minimal..High` set a budget. Nested
        // under `generationConfig.thinkingConfig`.
        if let Some(effort) = cfg.reasoning_effort {
            if effort != ReasoningEffort::Off {
                generation_config.insert(
                    "thinkingConfig".into(),
                    json!({ "thinkingBudget": gemini_budget(effort) }),
                );
            }
        }
        obj.insert("generationConfig".into(), Value::Object(generation_config));

        if let Some(tools) = &cfg.tools
            && !tools.is_empty()
        {
            let declarations: Vec<Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.function.name,
                        "description": t.function.description,
                        // Gemini rejects additionalProperties anywhere in the schema.
                        "parameters": strip_additional_properties(&t.function.parameters),
                    })
                })
                .collect();
            obj.insert("tools".into(), json!([{ "functionDeclarations": declarations }]));
        }
        for (k, v) in &cfg.extra {
            obj.insert(k.clone(), v.clone());
        }

        // Canonical base "https://generativelanguage.googleapis.com" →
        // "/v1beta/models/{model}:method"; a base ending in "/v1beta" keeps it.
        let base = cfg.base_url.trim_end_matches('/');
        let prefix = if base.ends_with("/v1beta") || base.ends_with("/v1") {
            base.to_string()
        } else {
            format!("{base}/v1beta")
        };
        let method = if stream { "streamGenerateContent?alt=sse" } else { "generateContent" };
        let url = format!("{prefix}/models/{}:{method}", cfg.model);

        let mut headers = Vec::new();
        if let Some(key) = api_key {
            headers.push(("x-goog-api-key".into(), key.to_string()));
        }

        RenderedRequest { url, headers, body }
    }

    fn parse_stream_line(&self, line: &str, state: &mut StreamParseState) -> Vec<LlmStreamEvent> {
        let Some(data) = line.strip_prefix("data: ") else {
            return Vec::new();
        };
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            tracing::warn!(data = %data, "failed to parse Gemini streaming frame");
            return Vec::new();
        };
        let mut events = Vec::new();
        if let Some(model) = v.get("modelVersion").and_then(Value::as_str) {
            events.push(LlmStreamEvent::Meta { id: None, model: Some(model.to_string()) });
        }
        if let Some(candidate) = v.pointer("/candidates/0") {
            for part in candidate
                .pointer("/content/parts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    events.push(LlmStreamEvent::TextDelta(text.to_string()));
                }
                if let Some(call) = part.get("functionCall") {
                    let index = state.next_tool_index;
                    state.next_tool_index += 1;
                    events.push(LlmStreamEvent::ToolCallDelta {
                        index,
                        id: Some(
                            call.get("id")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("call_{index}")),
                        ),
                        name: call.get("name").and_then(Value::as_str).map(str::to_string),
                        args_delta: call.get("args").map(|a| a.to_string()),
                    });
                }
            }
            if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
                events.push(LlmStreamEvent::Finish(Some(reason.to_string())));
            }
        }
        if let Some(meta) = v.get("usageMetadata") {
            let prompt = meta.get("promptTokenCount").and_then(Value::as_u64).unwrap_or(0) as u32;
            let completion =
                meta.get("candidatesTokenCount").and_then(Value::as_u64).unwrap_or(0) as u32;
            let total = meta
                .get("totalTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or((prompt + completion) as u64) as u32;
            events.push(LlmStreamEvent::Usage(Usage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: total,
                prompt_tokens_details: None,
                completion_tokens_details: None,
            }));
        }
        events
    }

    fn parse_response(&self, body: Value) -> Result<ParsedCompletion, String> {
        let candidate = body.pointer("/candidates/0").ok_or("no candidates in response")?;
        let mut text = String::new();
        let mut tool_calls = Vec::new();
        for (i, part) in candidate
            .pointer("/content/parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            if let Some(t) = part.get("text").and_then(Value::as_str) {
                text.push_str(t);
            }
            if let Some(call) = part.get("functionCall") {
                tool_calls.push(ToolCall {
                    id: call
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("call_{i}")),
                    call_type: "function".into(),
                    function: FunctionCall {
                        name: call
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        arguments: call.get("args").map(|a| a.to_string()).unwrap_or_else(|| "{}".into()),
                    },
                });
            }
        }
        let usage = body.get("usageMetadata").map(|meta| {
            let prompt = meta.get("promptTokenCount").and_then(Value::as_u64).unwrap_or(0) as u32;
            let completion =
                meta.get("candidatesTokenCount").and_then(Value::as_u64).unwrap_or(0) as u32;
            Usage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: meta
                    .get("totalTokenCount")
                    .and_then(Value::as_u64)
                    .unwrap_or((prompt + completion) as u64) as u32,
                prompt_tokens_details: None,
                completion_tokens_details: None,
            }
        });
        let message = ChatMessage {
            role: MessageRole::Assistant,
            content: if text.is_empty() { None } else { Some(text) },
            name: None,
            tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls.clone()) },
            tool_call_id: None,
            function_call: None,
        };
        Ok(ParsedCompletion {
            id: body
                .get("responseId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            model: body
                .get("modelVersion")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            message,
            finish_reason: candidate
                .get("finishReason")
                .and_then(Value::as_str)
                .map(str::to_string),
            usage,
        })
    }
}

// =============================================================================
// Tests — the hard transforms, pinned (B-G1 unit list from the fix plan)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(kind: Option<AdapterKind>) -> LlmClientConfig {
        LlmClientConfig {
            provider_kind: kind,
            ..Default::default()
        }
    }

    fn convo() -> Vec<ChatMessage> {
        vec![
            ChatMessage::system("Be brief."),
            ChatMessage::user("hi"),
            ChatMessage::assistant("hello"),
            ChatMessage::user("weather?"),
        ]
    }

    fn tool_convo() -> Vec<ChatMessage> {
        let mut assistant = ChatMessage::assistant("");
        assistant.content = None;
        assistant.tool_calls = Some(vec![ToolCall {
            id: "call_1".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "get_weather".into(),
                arguments: r#"{"city":"Paris"}"#.into(),
            },
        }]);
        vec![
            ChatMessage::system("Be brief."),
            ChatMessage::user("weather in Paris?"),
            assistant,
            ChatMessage::tool("call_1", r#"{"temp_c": 21}"#),
        ]
    }

    // --- selection ---

    #[test]
    fn explicit_provider_kind_wins() {
        let mut c = cfg(Some(AdapterKind::Anthropic));
        c.base_url = "https://my-proxy.example.com/v1".into();
        assert_eq!(select_adapter(&c).kind(), AdapterKind::Anthropic);
    }

    #[test]
    fn canonical_hosts_inferred_proxies_default_openai() {
        let mut c = cfg(None);
        c.base_url = "https://api.anthropic.com".into();
        assert_eq!(select_adapter(&c).kind(), AdapterKind::Anthropic);
        c.base_url = "https://generativelanguage.googleapis.com".into();
        assert_eq!(select_adapter(&c).kind(), AdapterKind::Gemini);
        // An OpenAI-compatible proxy in front of Claude is OpenAI wire format.
        c.base_url = "https://litellm.internal/v1".into();
        assert_eq!(select_adapter(&c).kind(), AdapterKind::OpenAi);
    }

    // --- OpenAI: byte-compat with the legacy request ---

    #[test]
    fn openai_keeps_system_inline_and_defaults_max_completion_tokens() {
        let r = OpenAiAdapter.render_request(&convo(), &cfg(None), false, Some("sk-test"));
        assert!(r.url.ends_with("/chat/completions"));
        assert_eq!(r.body["messages"][0]["role"], "system");
        assert_eq!(r.body["max_completion_tokens"], 4096);
        assert!(r.body.get("max_tokens").is_none());
        assert!(r
            .headers
            .iter()
            .any(|(k, v)| k == "Authorization" && v == "Bearer sk-test"));
    }

    #[test]
    fn openai_explicit_max_tokens_respected() {
        let mut c = cfg(None);
        c.max_tokens = Some(256);
        let r = OpenAiAdapter.render_request(&convo(), &c, true, None);
        assert_eq!(r.body["max_tokens"], 256);
        assert!(r.body.get("max_completion_tokens").is_none());
        assert_eq!(r.body["stream"], true);
    }

    // --- D1 reasoning_effort: enum logic (STEP 0) ---

    #[test]
    fn reasoning_effort_serde_roundtrip_lowercase() {
        for (v, s) in [
            (ReasoningEffort::Off, "\"off\""),
            (ReasoningEffort::Minimal, "\"minimal\""),
            (ReasoningEffort::Low, "\"low\""),
            (ReasoningEffort::Medium, "\"medium\""),
            (ReasoningEffort::High, "\"high\""),
        ] {
            assert_eq!(serde_json::to_string(&v).unwrap(), s);
            assert_eq!(serde_json::from_str::<ReasoningEffort>(s).unwrap(), v);
            assert_eq!(v.as_str(), s.trim_matches('"'));
        }
        // A typo fails to deserialize (typed enum catches it, like AdapterKind).
        assert!(serde_json::from_str::<ReasoningEffort>("\"minimial\"").is_err());
        assert!(serde_json::from_str::<ReasoningEffort>("\"xhigh\"").is_err());
    }

    #[test]
    fn reasoning_effort_floor_clamp_resolve() {
        // Ordinary models floor at Off; adaptive-only models floor at Low.
        assert_eq!(ReasoningEffort::floor_for_model("gpt-4o-mini"), ReasoningEffort::Off);
        assert_eq!(ReasoningEffort::floor_for_model("llama3.2:1b"), ReasoningEffort::Off);
        assert_eq!(ReasoningEffort::floor_for_model("claude-opus-4-8"), ReasoningEffort::Low);
        assert_eq!(ReasoningEffort::floor_for_model("fable-5"), ReasoningEffort::Low);
        // clamp raises to the floor but never lowers a higher request.
        assert_eq!(ReasoningEffort::Off.clamp_to_floor(ReasoningEffort::Low), ReasoningEffort::Low);
        assert_eq!(ReasoningEffort::High.clamp_to_floor(ReasoningEffort::Low), ReasoningEffort::High);
        assert_eq!(ReasoningEffort::Off.clamp_to_floor(ReasoningEffort::Off), ReasoningEffort::Off);
        // resolve = (applied, floor); None stays None (vendor default).
        assert_eq!(
            ReasoningEffort::resolve("gpt-4o-mini", Some(ReasoningEffort::Off)),
            (Some(ReasoningEffort::Off), ReasoningEffort::Off)
        );
        assert_eq!(
            ReasoningEffort::resolve("claude-opus-4-8", Some(ReasoningEffort::Off)),
            (Some(ReasoningEffort::Low), ReasoningEffort::Low)
        );
        assert_eq!(
            ReasoningEffort::resolve("gpt-4o-mini", None),
            (None, ReasoningEffort::Off)
        );
    }

    // --- D1 reasoning_effort: per-vendor request mapping (STEP 1) ---

    #[test]
    fn openai_maps_reasoning_effort_flat_string() {
        // Some(Low) → flat top-level `reasoning_effort: "low"`; Off → key absent;
        // None → key absent. AT MOST ONE param: never a nested reasoning object.
        let mut c = cfg(None);
        c.reasoning_effort = Some(ReasoningEffort::Low);
        let r = OpenAiAdapter.render_request(&convo(), &c, false, None);
        assert_eq!(r.body["reasoning_effort"], "low");
        assert!(r.body.get("reasoning").is_none());
        assert!(r.body.get("thinking").is_none());

        // Off → NO param (a fast/non-reasoning model 400s on any thinking param;
        // live-verified against ollama "does not support thinking").
        c.reasoning_effort = Some(ReasoningEffort::Off);
        let r = OpenAiAdapter.render_request(&convo(), &c, false, None);
        assert!(r.body.get("reasoning_effort").is_none(), "Off emits no param");

        c.reasoning_effort = None;
        let r = OpenAiAdapter.render_request(&convo(), &c, false, None);
        assert!(r.body.get("reasoning_effort").is_none());
    }

    #[test]
    fn anthropic_maps_reasoning_effort_thinking_400_safe() {
        // Voice cap (256): a valid budget needs 1024 <= budget < max_tokens, which
        // is unsatisfiable at 256 → emit NO thinking block (never a guaranteed 400).
        let mut c = cfg(Some(AdapterKind::Anthropic));
        c.max_tokens = Some(256);
        c.reasoning_effort = Some(ReasoningEffort::High);
        let r = AnthropicAdapter.render_request(&convo(), &c, false, None);
        assert!(
            r.body.get("thinking").is_none(),
            "budget can't satisfy [1024, 256) → no thinking block, not a 400"
        );

        // Room for a valid budget → enabled with 1024 <= budget < max_tokens.
        c.max_tokens = Some(4000);
        let r = AnthropicAdapter.render_request(&convo(), &c, false, None);
        assert_eq!(r.body["thinking"]["type"], "enabled");
        let budget = r.body["thinking"]["budget_tokens"].as_u64().unwrap();
        assert!((1024..4000).contains(&(budget as u32)), "budget {budget} in [1024,4000)");
        assert!(r.body.get("reasoning_effort").is_none(), "exactly one param");

        // Adaptive-only model (Opus 4.8) rejects the enabled form → emit nothing.
        c.model = "claude-opus-4-8".to_string();
        c.reasoning_effort = Some(ReasoningEffort::High);
        let r = AnthropicAdapter.render_request(&convo(), &c, false, None);
        assert!(
            r.body.get("thinking").is_none(),
            "adaptive-only model: never the enabled+budget form (it 400s)"
        );

        // Off / None → no thinking key (default no extended thinking).
        c.model = "claude-sonnet-4-5".to_string();
        c.reasoning_effort = Some(ReasoningEffort::Off);
        let r = AnthropicAdapter.render_request(&convo(), &c, false, None);
        assert!(r.body.get("thinking").is_none());
        c.reasoning_effort = None;
        let r = AnthropicAdapter.render_request(&convo(), &c, false, None);
        assert!(r.body.get("thinking").is_none());
    }

    #[test]
    fn anthropic_thinking_suppresses_temperature_and_top_p() {
        // Sampling params + extended thinking is a hard 400. When a thinking block
        // is emitted, temperature/top_p must be dropped (review fix).
        let mut c = cfg(Some(AdapterKind::Anthropic));
        c.model = "claude-sonnet-4-5".to_string();
        c.max_tokens = Some(4000);
        c.temperature = Some(0.7);
        c.top_p = Some(0.9);
        c.reasoning_effort = Some(ReasoningEffort::High);
        let r = AnthropicAdapter.render_request(&convo(), &c, false, None);
        assert_eq!(r.body["thinking"]["type"], "enabled", "thinking is on");
        assert!(
            r.body.get("temperature").is_none(),
            "temperature must be suppressed when thinking is enabled (else 400)"
        );
        assert!(
            r.body.get("top_p").is_none(),
            "top_p must be suppressed when thinking is enabled (else 400)"
        );

        // No thinking (effort suppressed by a low max_tokens) → sampling params stay.
        c.max_tokens = Some(256);
        let r = AnthropicAdapter.render_request(&convo(), &c, false, None);
        assert!(r.body.get("thinking").is_none());
        assert!(
            (r.body["temperature"].as_f64().unwrap() - 0.7).abs() < 1e-3,
            "no thinking ⇒ temperature kept"
        );
        assert!(
            (r.body["top_p"].as_f64().unwrap() - 0.9).abs() < 1e-3,
            "no thinking ⇒ top_p kept"
        );
    }

    #[test]
    fn gemini_maps_reasoning_effort_only_thinking_budget_never_level_pair() {
        let mut c = cfg(Some(AdapterKind::Gemini));
        // Off → NO thinkingConfig (a non-thinking model rejects it).
        c.reasoning_effort = Some(ReasoningEffort::Off);
        let r = GeminiAdapter.render_request(&convo(), &c, false, None);
        assert!(r.body["generationConfig"].get("thinkingConfig").is_none());

        // High → only thinkingBudget, NEVER a thinkingLevel pair (400 tripwire).
        c.reasoning_effort = Some(ReasoningEffort::High);
        let r = GeminiAdapter.render_request(&convo(), &c, false, None);
        let tc = &r.body["generationConfig"]["thinkingConfig"];
        assert_eq!(tc["thinkingBudget"], 24576);
        assert!(tc.get("thinkingLevel").is_none());
        assert!(tc.get("thinking_level").is_none());

        c.reasoning_effort = None;
        let r = GeminiAdapter.render_request(&convo(), &c, false, None);
        assert!(r.body["generationConfig"].get("thinkingConfig").is_none());
    }

    // --- Anthropic: the hard transforms ---

    #[test]
    fn anthropic_extracts_system_to_top_level_param() {
        let r = AnthropicAdapter.render_request(&convo(), &cfg(None), false, Some("sk-ant"));
        assert_eq!(r.body["system"], "Be brief.");
        // No system role inside messages.
        for msg in r.body["messages"].as_array().unwrap() {
            assert_ne!(msg["role"], "system");
        }
        assert!(r.url.ends_with("/v1/messages"));
        assert!(r.headers.iter().any(|(k, v)| k == "x-api-key" && v == "sk-ant"));
        assert!(r
            .headers
            .iter()
            .any(|(k, v)| k == "anthropic-version" && v == ANTHROPIC_VERSION));
    }

    #[test]
    fn anthropic_request_includes_max_tokens() {
        // Anthropic REQUIRES max_tokens — the legacy OpenAI field name
        // (max_completion_tokens) would 400 every call (review B1).
        let r = AnthropicAdapter.render_request(&convo(), &cfg(None), false, None);
        assert_eq!(r.body["max_tokens"], ANTHROPIC_DEFAULT_MAX_TOKENS);
        assert!(r.body.get("max_completion_tokens").is_none());
        let mut c = cfg(None);
        c.max_tokens = Some(256);
        let r = AnthropicAdapter.render_request(&convo(), &c, false, None);
        assert_eq!(r.body["max_tokens"], 256);
    }

    #[test]
    fn anthropic_single_system_message_becomes_user_not_extracted() {
        let only_system = vec![ChatMessage::system("Greet the user.")];
        let r = AnthropicAdapter.render_request(&only_system, &cfg(None), false, None);
        assert!(r.body.get("system").is_none());
        let messages = r.body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"][0]["text"], "Greet the user.");
    }

    #[test]
    fn anthropic_tool_message_becomes_user_tool_result_block() {
        let r = AnthropicAdapter.render_request(&tool_convo(), &cfg(None), false, None);
        let messages = r.body["messages"].as_array().unwrap();
        let last = messages.last().unwrap();
        assert_eq!(last["role"], "user");
        assert_eq!(last["content"][0]["type"], "tool_result");
        assert_eq!(last["content"][0]["tool_use_id"], "call_1");
    }

    #[test]
    fn anthropic_assistant_tool_calls_become_tool_use_with_parsed_object_input() {
        let r = AnthropicAdapter.render_request(&tool_convo(), &cfg(None), false, None);
        let messages = r.body["messages"].as_array().unwrap();
        let assistant = &messages[1];
        assert_eq!(assistant["role"], "assistant");
        let block = &assistant["content"][0];
        assert_eq!(block["type"], "tool_use");
        assert_eq!(block["name"], "get_weather");
        // Arguments are a PARSED OBJECT, not the JSON string.
        assert_eq!(block["input"]["city"], "Paris");
    }

    #[test]
    fn anthropic_merges_consecutive_same_role() {
        let messages = vec![
            ChatMessage::user("first"),
            ChatMessage::user("second"),
            ChatMessage::assistant("ok"),
        ];
        let r = AnthropicAdapter.render_request(&messages, &cfg(None), false, None);
        let rendered = r.body["messages"].as_array().unwrap();
        assert_eq!(rendered.len(), 2, "consecutive user turns must merge");
        assert_eq!(rendered[0]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn anthropic_empty_content_becomes_placeholder() {
        let mut empty = ChatMessage::user("");
        empty.content = Some(String::new());
        let r = AnthropicAdapter.render_request(&[empty], &cfg(None), false, None);
        assert_eq!(r.body["messages"][0]["content"][0]["text"], "(empty)");
    }

    #[test]
    fn anthropic_stream_parse_full_sequence() {
        let mut st = StreamParseState::default();
        let a = AnthropicAdapter;
        let lines = [
            "event: message_start",
            r#"data: {"type":"message_start","message":{"id":"msg_1","model":"claude-x","usage":{"input_tokens":10}}}"#,
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
            r#"data: {"type":"message_stop"}"#,
        ];
        let events: Vec<LlmStreamEvent> =
            lines.iter().flat_map(|l| a.parse_stream_line(l, &mut st)).collect();
        assert!(matches!(&events[0], LlmStreamEvent::Meta { id: Some(id), .. } if id == "msg_1"));
        assert!(matches!(&events[1], LlmStreamEvent::TextDelta(t) if t == "Hello"));
        assert!(matches!(&events[2], LlmStreamEvent::Finish(Some(r)) if r == "end_turn"));
        assert!(
            matches!(&events[3], LlmStreamEvent::Usage(u) if u.prompt_tokens == 10 && u.completion_tokens == 5 && u.total_tokens == 15)
        );
        assert!(matches!(&events[4], LlmStreamEvent::StreamEnd));
    }

    #[test]
    fn anthropic_parse_response_normalizes() {
        let body = serde_json::json!({
            "id": "msg_9", "model": "claude-x", "stop_reason": "tool_use",
            "content": [
                {"type": "text", "text": "Let me check."},
                {"type": "tool_use", "id": "tu_1", "name": "get_weather", "input": {"city": "Paris"}}
            ],
            "usage": {"input_tokens": 7, "output_tokens": 9}
        });
        let p = AnthropicAdapter.parse_response(body).unwrap();
        assert_eq!(p.id, "msg_9");
        assert_eq!(p.message.content.as_deref(), Some("Let me check."));
        let calls = p.message.tool_calls.as_ref().unwrap();
        assert_eq!(calls[0].function.name, "get_weather");
        assert_eq!(p.finish_reason.as_deref(), Some("tool_use"));
        assert_eq!(p.usage.unwrap().total_tokens, 16);
    }

    // --- Gemini: the hard transforms ---

    #[test]
    fn gemini_assistant_role_becomes_model_and_system_separate() {
        let r = GeminiAdapter.render_request(&convo(), &cfg(None), false, Some("g-key"));
        assert_eq!(r.body["systemInstruction"]["parts"][0]["text"], "Be brief.");
        let contents = r.body["contents"].as_array().unwrap();
        assert_eq!(contents[1]["role"], "model");
        assert!(r.url.contains(":generateContent"));
        assert!(r.headers.iter().any(|(k, v)| k == "x-goog-api-key" && v == "g-key"));
    }

    #[test]
    fn gemini_request_maps_max_output_tokens() {
        let mut c = cfg(None);
        c.max_tokens = Some(256);
        let r = GeminiAdapter.render_request(&convo(), &c, true, None);
        assert_eq!(r.body["generationConfig"]["maxOutputTokens"], 256);
        assert!(r.url.ends_with(":streamGenerateContent?alt=sse"));
    }

    #[test]
    fn gemini_tool_result_becomes_function_response_with_name_from_id_map() {
        let r = GeminiAdapter.render_request(&tool_convo(), &cfg(None), false, None);
        let contents = r.body["contents"].as_array().unwrap();
        let last = contents.last().unwrap();
        assert_eq!(last["role"], "user");
        // Name resolved from the call_1 → get_weather map, NOT the id.
        assert_eq!(last["parts"][0]["functionResponse"]["name"], "get_weather");
        assert_eq!(last["parts"][0]["functionResponse"]["response"]["temp_c"], 21);
    }

    #[test]
    fn gemini_non_dict_result_wrapped_as_value_object() {
        let mut messages = tool_convo();
        messages.last_mut().unwrap().content = Some("sunny".into());
        let r = GeminiAdapter.render_request(&messages, &cfg(None), false, None);
        let last = r.body["contents"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(last["parts"][0]["functionResponse"]["response"]["value"], "sunny");
    }

    #[test]
    fn gemini_unmapped_tool_id_falls_back() {
        let messages = vec![ChatMessage::tool("orphan_id", r#"{"x":1}"#)];
        let r = GeminiAdapter.render_request(&messages, &cfg(None), false, None);
        let first = &r.body["contents"][0];
        assert_eq!(first["parts"][0]["functionResponse"]["name"], "tool_call_result");
    }

    #[test]
    fn gemini_strips_additional_properties_from_tool_schema() {
        let mut c = cfg(None);
        c.tools = Some(vec![ToolDefinition::function(
            "f",
            "d",
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": { "x": { "type": "object", "additionalProperties": false } }
            }),
        )]);
        let r = GeminiAdapter.render_request(&convo(), &c, false, None);
        let schema = &r.body["tools"][0]["functionDeclarations"][0]["parameters"];
        assert!(schema.get("additionalProperties").is_none());
        assert!(schema["properties"]["x"].get("additionalProperties").is_none());
    }

    #[test]
    fn gemini_stream_parse_text_call_and_usage() {
        let mut st = StreamParseState::default();
        let g = GeminiAdapter;
        let line = r#"data: {"candidates":[{"content":{"parts":[{"text":"Hi"},{"functionCall":{"name":"f","args":{"x":1}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":4,"totalTokenCount":7}}"#;
        let events = g.parse_stream_line(line, &mut st);
        assert!(matches!(&events[0], LlmStreamEvent::TextDelta(t) if t == "Hi"));
        assert!(
            matches!(&events[1], LlmStreamEvent::ToolCallDelta { name: Some(n), args_delta: Some(a), .. } if n == "f" && a.contains("\"x\":1"))
        );
        assert!(matches!(&events[2], LlmStreamEvent::Finish(Some(r)) if r == "STOP"));
        assert!(matches!(&events[3], LlmStreamEvent::Usage(u) if u.total_tokens == 7));
    }

    // --- self-verified review pins (wf_e1bdd231; verifiers hit session cap,
    //     candidates verified + fixed by hand) ---

    #[test]
    fn anthropic_leading_assistant_after_history_trim_gets_user_guard() {
        // ConversationHistory::add trims the OLDEST non-system message, so a
        // wrapped window can start with an assistant turn — Anthropic requires
        // messages[0] to be user, else every call 400s.
        let messages = vec![
            ChatMessage::assistant("…earlier reply…"),
            ChatMessage::user("next question"),
        ];
        let r = AnthropicAdapter.render_request(&messages, &cfg(None), false, None);
        let rendered = r.body["messages"].as_array().unwrap();
        assert_eq!(rendered[0]["role"], "user");
        assert_eq!(rendered[0]["content"][0]["text"], "(history truncated)");
        assert_eq!(rendered[1]["role"], "assistant");
    }

    #[test]
    fn anthropic_mid_stream_error_frame_surfaces() {
        let mut st = StreamParseState::default();
        let events = AnthropicAdapter.parse_stream_line(
            r#"data: {"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
            &mut st,
        );
        assert!(matches!(&events[0], LlmStreamEvent::Error(d) if d.contains("overloaded_error")));
    }

    #[test]
    fn openai_meta_emitted_once_per_stream() {
        let mut st = StreamParseState::default();
        let o = OpenAiAdapter;
        let line = r#"data: {"id":"c1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":"a"},"finish_reason":null}]}"#;
        let first = o.parse_stream_line(line, &mut st);
        let second = o.parse_stream_line(line, &mut st);
        assert!(first.iter().any(|e| matches!(e, LlmStreamEvent::Meta { .. })));
        assert!(
            !second.iter().any(|e| matches!(e, LlmStreamEvent::Meta { .. })),
            "Meta must not re-allocate on every chunk (hot path)"
        );
    }

    #[test]
    fn openai_error_data_line_surfaces() {
        let mut st = StreamParseState::default();
        let events = OpenAiAdapter.parse_stream_line(
            r#"data: {"error":{"message":"rate limited","code":429}}"#,
            &mut st,
        );
        assert!(matches!(&events[0], LlmStreamEvent::Error(d) if d.contains("rate limited")));
    }

    #[test]
    fn gemini_empty_assistant_renders_placeholder_part() {
        let mut empty = ChatMessage::assistant("");
        empty.content = None;
        let r = GeminiAdapter.render_request(&[ChatMessage::user("q"), empty], &cfg(None), false, None);
        let contents = r.body["contents"].as_array().unwrap();
        assert_eq!(contents[1]["parts"][0]["text"], "(empty)");
    }

    #[test]
    fn adapter_inference_rejects_lookalike_hosts() {
        // Substring matching would select Anthropic here — and the env-key
        // fallback would then send ANTHROPIC_API_KEY to the attacker's host.
        let mut c = cfg(None);
        c.base_url = "https://api.anthropic.com.evil.com/v1".into();
        assert_eq!(select_adapter(&c).kind(), AdapterKind::OpenAi);
        c.base_url = "https://generativelanguage.googleapis.com.evil.com".into();
        assert_eq!(select_adapter(&c).kind(), AdapterKind::OpenAi);
    }

    // --- purity: render never mutates the canonical context ---

    #[test]
    fn render_request_does_not_mutate_stored_context() {
        let messages = tool_convo();
        let snapshot = serde_json::to_string(&messages).unwrap();
        for kind in [AdapterKind::OpenAi, AdapterKind::Anthropic, AdapterKind::Gemini] {
            let _ = adapter_for(kind).render_request(&messages, &cfg(None), true, Some("k"));
        }
        assert_eq!(serde_json::to_string(&messages).unwrap(), snapshot);
    }

    // --- OpenAI stream parity (incl. [DONE]) ---

    #[test]
    fn openai_stream_parse_done_and_delta() {
        let mut st = StreamParseState::default();
        let o = OpenAiAdapter;
        let line = r#"data: {"id":"c1","object":"chat.completion.chunk","created":1,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"content":"Hey"},"finish_reason":null}]}"#;
        let events = o.parse_stream_line(line, &mut st);
        assert!(matches!(&events[0], LlmStreamEvent::Meta { id: Some(id), .. } if id == "c1"));
        assert!(matches!(&events[1], LlmStreamEvent::TextDelta(t) if t == "Hey"));
        assert!(matches!(
            o.parse_stream_line("data: [DONE]", &mut st).as_slice(),
            [LlmStreamEvent::StreamEnd]
        ));
    }
}
