//! The agent leg: one budprompt turn per user turn, through budgateway `/v1/responses` (spec 025 D-2).
//!
//! WaaV never runs the agent. It POSTs `{prompt: {id, version, variables}, input, conversation,
//! stream: true, bud_channel: "voice"}` to the Bud gateway with the CALLER's credential (S-1);
//! budgateway authorizes the agent alias, applies rate limits and agent pricing, and budprompt runs the
//! prompt, tools and governance exactly as for a text caller. This module is the HTTP + SSE client
//! and the mapping from the Responses stream to the few events a voice turn needs.
//!
//! Everything here is pure except [`AgentBrain`]'s two HTTP calls, so the stream parsing, the event
//! mapping and the failure classification (FRD §5.8) are unit-tested without a network.

use std::time::Duration;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{Map, Value, json};

/// One SSE message may not exceed this many bytes. budprompt's largest frame is a terminal
/// `response.completed` carrying the whole output; a reply bounded by `max_output_tokens` is far
/// below it. Unbounded accumulation of a streamed body is how a misbehaving upstream OOMs a pod.
pub const MAX_SSE_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// Bound on an error body read for classification.
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// What a voice turn learns from the Responses stream.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// The run exists: its id is what `bud_truncate` and `/cancel` name.
    Created {
        response_id: String,
        conversation_id: Option<String>,
    },
    /// Assistant text, as generated.
    TextDelta {
        item_id: Option<String>,
        delta: String,
    },
    /// A server-side tool started (MCP, web search, code interpreter, a delegated agent).
    ToolStarted {
        item_id: String,
        name: String,
        kind: String,
    },
    /// That tool finished. `error`: what the item said went wrong, when it failed and said so.
    ToolFinished {
        item_id: String,
        name: String,
        ok: bool,
        error: Option<String>,
    },
    /// A function call finished being written. It is a tool of the agent's own unless the
    /// session declared a function of that name ([`ClientExtras::tools`]): then the client
    /// runs it and sends its output with a later run.
    FunctionCall {
        item_id: String,
        name: String,
        ok: bool,
        call_id: String,
        arguments: String,
    },
    /// budprompt holds a tool call until the caller approves it (spec 029 §7.3, an
    /// `mcp_approval_request` item): the stream stays open while it waits for the vote.
    /// `id` is the escalation's, `arguments` the call's JSON as a string, `expires_at` the
    /// unix time (seconds) after which budprompt declines it.
    ApprovalRequest {
        id: String,
        name: String,
        arguments: String,
        server_label: Option<String>,
        expires_at: Option<f64>,
    },
    /// The run completed.
    Completed {
        usage: Option<Value>,
        output: Vec<Value>,
    },
    /// The run failed after it started streaming.
    Failed { code: String, message: String },
    /// The run ended incomplete (cancelled, a governance withhold, max tokens).
    Incomplete {
        status: String,
        reason: Option<String>,
        output: Vec<Value>,
    },
}

/// Why a turn could not be served, classified the way the session must react (FRD §5.8).
#[derive(Debug, Clone, PartialEq)]
pub enum TurnFailure {
    /// 429 from budgateway or budprompt: speak the degradation message, the session continues.
    RateLimited(String),
    /// 401: a JWT expired (refreshable) or a key was revoked.
    Unauthorized(String),
    /// 403 `approval_required_foreground`: the turn needs a human approval (NG-3).
    ApprovalRequired(String),
    /// 403 for any other reason (policy denial).
    Forbidden(String),
    /// 400 on the agent's structured input: the variables are wrong.
    InvalidVariables(String),
    /// 404: the agent (or its version) is gone.
    AgentNotFound(String),
    /// Any other 4xx: a request this gateway built badly.
    BadRequest(String),
    /// 5xx, a failed run, or a broken stream.
    Upstream(String),
    /// The Bud gateway could not be reached at all.
    Transport(String),
    /// A bug in this gateway ended the turn (the turn task panicked).
    Internal(String),
}

impl TurnFailure {
    /// The `error.code` a client sees.
    pub fn code(&self) -> &'static str {
        match self {
            Self::RateLimited(_) => "rate_limit_exceeded",
            Self::Unauthorized(_) => "auth_expired",
            Self::ApprovalRequired(_) => "approval_required",
            Self::Forbidden(_) => "forbidden",
            Self::InvalidVariables(_) => "invalid_variables",
            Self::AgentNotFound(_) => "agent_not_found",
            Self::BadRequest(_) => "invalid_request",
            Self::Upstream(_) => "upstream_error",
            Self::Transport(_) => "upstream_unavailable",
            Self::Internal(_) => "internal_error",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::RateLimited(m)
            | Self::Unauthorized(m)
            | Self::ApprovalRequired(m)
            | Self::Forbidden(m)
            | Self::InvalidVariables(m)
            | Self::AgentNotFound(m)
            | Self::BadRequest(m)
            | Self::Upstream(m)
            | Self::Transport(m)
            | Self::Internal(m) => m,
        }
    }

    /// The failure a run reported mid-stream (`response.failed`, `error`): a run held for an
    /// approval only a designated person can give (`approval_required`, spec 029 §7.3) is the
    /// approval failure; any other is upstream.
    pub fn of_run(code: &str, message: &str) -> Self {
        if code == "approval_required" {
            return Self::ApprovalRequired(message.to_owned());
        }
        Self::Upstream(format!("{code}: {message}"))
    }

    /// Whether the turn may be tried again: only when the request never reached the agent, so no
    /// run exists that a retry could duplicate.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Transport(_))
    }

    /// Whether the caller hears the degradation message for it (a silent failure is dead air).
    pub fn speaks_degradation(&self) -> bool {
        matches!(
            self,
            Self::RateLimited(_)
                | Self::Upstream(_)
                | Self::Transport(_)
                | Self::BadRequest(_)
                | Self::Internal(_)
        )
    }
}

/// Classify a non-2xx answer from the Bud gateway by its status and OpenAI-shaped error body.
pub fn classify_http(status: u16, body: &str) -> TurnFailure {
    let err = serde_json::from_str::<Value>(body).ok();
    let field = |name: &str| -> Option<String> {
        let e = err.as_ref()?;
        e.get("error")
            .and_then(|x| x.get(name))
            .or_else(|| e.get(name))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let code = field("code").unwrap_or_default();
    let param = field("param").unwrap_or_default();
    let message = field("message")
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| format!("the Bud gateway answered {status}"));
    match status {
        429 => TurnFailure::RateLimited(message),
        401 => TurnFailure::Unauthorized(message),
        403 if code.contains("approval") => TurnFailure::ApprovalRequired(message),
        403 => TurnFailure::Forbidden(message),
        404 => TurnFailure::AgentNotFound(message),
        400 | 422
            if code.contains("variable")
                || param.starts_with("prompt.variables")
                || message.contains("variable") =>
        {
            TurnFailure::InvalidVariables(message)
        }
        400..=499 => TurnFailure::BadRequest(message),
        // The gateway could not reach the agent: the request never ran, so it may be retried.
        502..=504 => TurnFailure::Transport(message),
        500..=599 if never_reached(&message) => TurnFailure::Transport(message),
        _ => TurnFailure::Upstream(message),
    }
}

/// Whether a gateway error says the request never reached the agent (a connection-level failure on
/// its way to budprompt), as opposed to a run that started and failed.
fn never_reached(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    [
        "error sending request",
        "connection refused",
        "connection reset",
        "connection closed before",
    ]
    .iter()
    .any(|needle| m.contains(needle))
}

/// A `bud_truncate` for the previous turn (spec 025 D-9).
#[derive(Debug, Clone, PartialEq)]
pub struct Truncate {
    pub response_id: String,
    pub output_text: String,
}

/// The caller's vote on a held tool call (spec 029 §7.3): a request of its own, with the
/// session's bearer, continuing the turn that holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalVote {
    /// The turn's response id (`response.created`).
    pub previous_response_id: String,
    /// The hold's id (the escalation's).
    pub approval_request_id: String,
    pub approve: bool,
    pub reason: Option<String>,
}

impl ApprovalVote {
    /// The JSON body, without the `prompt` that [`AgentBrain::vote`] adds. No `conversation`:
    /// budprompt refuses it with `previous_response_id`.
    pub fn body(&self) -> Value {
        let mut response = json!({
            "type": "mcp_approval_response",
            "approval_request_id": self.approval_request_id,
            "approve": self.approve,
        });
        if let Some(reason) = self.reason.as_deref().filter(|r| !r.is_empty()) {
            response["reason"] = Value::from(reason);
        }
        json!({
            "previous_response_id": self.previous_response_id,
            "stream": false,
            "input": [response],
        })
    }
}

/// What a session adds to its agent's runs, where the agent lets a session do so (the GA
/// cascade's `session.tools` and `session.instructions`). All of it travels as the Responses
/// API's own request fields.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClientExtras {
    /// Function tools the client runs itself.
    pub tools: Vec<Value>,
    pub instructions: Option<String>,
    /// Input items sent ahead of the caller's words: a client tool's output, a system note.
    pub items: Vec<Value>,
}

/// One agent turn as the Bud gateway receives it.
#[derive(Debug, Clone)]
pub struct AgentTurnRequest {
    /// The name the caller addressed the agent by, without `prompt:` — budgateway looks up
    /// `prompt:{this}` in the caller's own key map, exactly as for a text caller.
    pub prompt_name: String,
    /// Pinned at connect (D-12).
    pub version: i64,
    pub variables: Option<Map<String, Value>>,
    pub input: String,
    /// `conv_voice_<session>`: named on the first turn, which is what creates it in budprompt.
    pub conversation_id: String,
    pub truncate: Option<Truncate>,
    pub session_id: String,
    pub turn_index: u64,
    pub extras: ClientExtras,
    /// The line the agent is to write before its final answer (`background.answer_marker`):
    /// sent as `bud_answer_marker` so budprompt tells the agent to write it. Only an
    /// interaction-mode background run sets it; a plain voice agent's body never carries it.
    pub answer_marker: Option<String>,
}

impl AgentTurnRequest {
    /// The JSON body.
    pub fn body(&self) -> Value {
        let mut prompt = json!({"id": self.prompt_name, "version": self.version.to_string()});
        if let Some(vars) = self.variables.as_ref().filter(|v| !v.is_empty()) {
            prompt["variables"] = Value::Object(vars.clone());
        }
        let mut body = json!({
            "prompt": prompt,
            "input": self.input,
            "conversation": self.conversation_id,
            "stream": true,
            "store": true,
            "bud_channel": "voice",
            "metadata": {
                "voice_session_id": self.session_id,
                "voice_turn_index": self.turn_index.to_string(),
            },
        });
        if let Some(t) = &self.truncate {
            body["bud_truncate"] =
                json!({"response_id": t.response_id, "output_text": t.output_text});
        }
        let extras = &self.extras;
        // A run the client asked for with no words of the caller has a list for its input.
        if !extras.items.is_empty() || (self.input.is_empty() && extras.instructions.is_some()) {
            let mut input = extras.items.clone();
            if !self.input.is_empty() {
                input.push(json!({"role": "user", "content": self.input}));
            }
            body["input"] = Value::Array(input);
        }
        if !extras.tools.is_empty() {
            body["tools"] = Value::Array(extras.tools.clone());
        }
        if let Some(instructions) = &extras.instructions {
            body["instructions"] = Value::from(instructions.as_str());
        }
        if let Some(marker) = self.answer_marker.as_deref().filter(|m| !m.is_empty()) {
            body["bud_answer_marker"] = Value::from(marker);
        }
        body
    }
}

// =============================================================================================
// SSE
// =============================================================================================

/// Incremental `text/event-stream` decoder: bytes in, `(event, data)` messages out.
#[derive(Debug, Default)]
pub struct SseDecoder {
    line: Vec<u8>,
    event: Option<String>,
    data: String,
    overflow: bool,
}

impl SseDecoder {
    /// Feed bytes; returns every message they completed. `Err` when one message exceeds
    /// [`MAX_SSE_MESSAGE_BYTES`] — the stream is then unusable and the turn fails.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<(Option<String>, String)>, String> {
        let mut out = Vec::new();
        for &b in bytes {
            if b == b'\n' {
                let mut line = std::mem::take(&mut self.line);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.line_done(&line, &mut out);
            } else {
                self.line.push(b);
            }
            if self.line.len() + self.data.len() > MAX_SSE_MESSAGE_BYTES {
                self.overflow = true;
            }
        }
        if self.overflow {
            return Err(format!(
                "an SSE message exceeded {MAX_SSE_MESSAGE_BYTES} bytes"
            ));
        }
        Ok(out)
    }

    fn line_done(&mut self, line: &[u8], out: &mut Vec<(Option<String>, String)>) {
        if line.is_empty() {
            if !self.data.is_empty() {
                out.push((self.event.take(), std::mem::take(&mut self.data)));
            }
            self.event = None;
            return;
        }
        let line = String::from_utf8_lossy(line);
        if line.starts_with(':') {
            return; // comment / keep-alive
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line.as_ref(), ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(value);
            }
            _ => {}
        }
    }
}

const TOOL_ITEM_TYPES: &[&str] = &[
    "mcp_call",
    "function_call",
    "web_search_call",
    "code_interpreter_call",
    "shell_call",
    "file_search_call",
];

fn tool_name(item: &Value) -> String {
    item.get("name")
        .and_then(Value::as_str)
        .or_else(|| item.get("server_label").and_then(Value::as_str))
        .or_else(|| item.get("type").and_then(Value::as_str))
        .unwrap_or("tool")
        .to_string()
}

/// What a finished tool item says went wrong: its `error` text, an error object's `message`, or
/// the object itself. `None` when it says nothing.
fn tool_error(item: &Value) -> Option<String> {
    let error = item.get("error").filter(|e| !e.is_null())?;
    let text = match error {
        Value::String(text) => text.clone(),
        other => other
            .get("message")
            .and_then(Value::as_str)
            .map_or_else(|| other.to_string(), str::to_string),
    };
    (!text.trim().is_empty()).then_some(text)
}

fn output_of(response: Option<&Value>) -> Vec<Value> {
    response
        .and_then(|r| r.get("output"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Map one SSE message to what a voice turn needs. `None` for everything else (lifecycle noise,
/// reasoning, argument deltas, `mcp_list_tools`).
pub fn map_event(event: Option<&str>, data: &str) -> Option<AgentEvent> {
    if data.trim() == "[DONE]" {
        return None;
    }
    let v: Value = serde_json::from_str(data).ok()?;
    let kind = v
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| event.map(str::to_string))?;
    let response = v.get("response");
    match kind.as_str() {
        "response.created" | "response.in_progress" => {
            let r = response?;
            let id = r.get("id").and_then(Value::as_str)?.to_string();
            let conversation_id = r
                .get("conversation")
                .and_then(|c| c.get("id").or(Some(c)))
                .and_then(Value::as_str)
                .map(str::to_string);
            Some(AgentEvent::Created {
                response_id: id,
                conversation_id,
            })
        }
        "response.output_text.delta" => {
            let delta = v.get("delta").and_then(Value::as_str)?.to_string();
            (!delta.is_empty()).then(|| AgentEvent::TextDelta {
                item_id: v.get("item_id").and_then(Value::as_str).map(str::to_string),
                delta,
            })
        }
        "response.output_item.added" | "response.output_item.done" => {
            let item = v.get("item")?;
            let item_type = item.get("type").and_then(Value::as_str)?;
            if item_type == "mcp_approval_request" {
                // Held until the caller votes (spec 029 §7.3); its `done` frame says nothing new.
                if kind != "response.output_item.added" {
                    return None;
                }
                let text = |key: &str| item.get(key).and_then(Value::as_str).map(str::to_string);
                return Some(AgentEvent::ApprovalRequest {
                    id: text("id")?,
                    name: tool_name(item),
                    arguments: text("arguments").unwrap_or_default(),
                    server_label: text("server_label"),
                    expires_at: item.get("bud_expires_at").and_then(Value::as_f64),
                });
            }
            if !TOOL_ITEM_TYPES.contains(&item_type) {
                return None;
            }
            let item_id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let name = tool_name(item);
            if kind == "response.output_item.added" {
                Some(AgentEvent::ToolStarted {
                    item_id,
                    name,
                    kind: item_type.to_string(),
                })
            } else {
                let failed = item.get("status").and_then(Value::as_str) == Some("failed")
                    || item.get("error").is_some_and(|e| !e.is_null());
                let text = |key: &str| item.get(key).and_then(Value::as_str).map(str::to_string);
                match text("call_id").filter(|_| item_type == "function_call") {
                    Some(call_id) => Some(AgentEvent::FunctionCall {
                        item_id,
                        name,
                        ok: !failed,
                        call_id,
                        arguments: text("arguments").unwrap_or_default(),
                    }),
                    None => Some(AgentEvent::ToolFinished {
                        item_id,
                        name,
                        ok: !failed,
                        error: tool_error(item),
                    }),
                }
            }
        }
        "response.completed" => Some(AgentEvent::Completed {
            usage: response
                .and_then(|r| r.get("usage"))
                .cloned()
                .filter(|u| !u.is_null()),
            output: output_of(response),
        }),
        "response.failed" | "error" => {
            let err = response
                .and_then(|r| r.get("error"))
                .or_else(|| v.get("error"))
                .unwrap_or(&v);
            Some(AgentEvent::Failed {
                code: err
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("server_error")
                    .to_string(),
                message: err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("the agent run failed")
                    .to_string(),
            })
        }
        "response.incomplete" => Some(AgentEvent::Incomplete {
            status: response
                .and_then(|r| r.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("incomplete")
                .to_string(),
            reason: response
                .and_then(|r| r.get("incomplete_details"))
                .and_then(|d| d.get("reason"))
                .and_then(Value::as_str)
                .map(str::to_string),
            output: output_of(response),
        }),
        _ => None,
    }
}

// =============================================================================================
// The HTTP client
// =============================================================================================

/// The client for the Bud gateway's `/v1/responses`.
#[derive(Clone)]
pub struct AgentBrain {
    http: reqwest::Client,
    /// e.g. `http://bud-budgateway:3000/v1` (`WAAV_LLM_BASE_URL`, operator-set; never the client's).
    base_url: String,
}

impl std::fmt::Debug for AgentBrain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentBrain")
            .field("base_url", &self.base_url)
            .finish()
    }
}

impl AgentBrain {
    pub fn new(base_url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            // No total timeout: a turn streams for as long as the agent talks. A dead stream is
            // detected by the read timeout below and by the session's own cancellation.
            .read_timeout(Duration::from_secs(120))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .unwrap_or_default();
        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    /// Open the turn's stream. A non-2xx answer is classified; the body is read, bounded.
    pub async fn open(
        &self,
        request: &AgentTurnRequest,
        bearer: &str,
        traceparent: Option<&str>,
    ) -> Result<
        impl Stream<Item = Result<AgentEvent, TurnFailure>> + Send + Unpin + 'static,
        TurnFailure,
    > {
        let mut req = self
            .http
            .post(format!("{}/responses", self.base_url))
            .bearer_auth(bearer)
            .header("accept", "text/event-stream")
            .json(&request.body());
        if let Some(tp) = traceparent {
            req = req.header("traceparent", tp);
        }
        let resp = req.send().await.map_err(|e| {
            TurnFailure::Transport(format!("the Bud gateway could not be reached: {e}"))
        })?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let body = read_bounded(resp, MAX_ERROR_BODY_BYTES).await;
            return Err(classify_http(status, &body));
        }
        Ok(Box::pin(events(resp.bytes_stream())))
    }

    /// Best-effort `POST /responses/{id}/cancel`. A run that already ended answers 409; that and
    /// every other failure is ignored — closing the stream already stopped the model. With the
    /// turn's `traceparent` it is a span of the turn's trace, not a trace of its own.
    pub async fn cancel(
        &self,
        response_id: &str,
        prompt_name: &str,
        bearer: &str,
        traceparent: Option<&str>,
    ) {
        let url = format!("{}/responses/{}/cancel", self.base_url, response_id);
        let mut req = self
            .http
            .post(url)
            .bearer_auth(bearer)
            .header("x-model-name", prompt_name)
            .timeout(Duration::from_secs(5));
        if let Some(tp) = traceparent {
            req = req.header("traceparent", tp);
        }
        let res = req.send().await;
        if let Err(e) = res {
            tracing::debug!(response_id, error = %e, "agent run cancel failed (best-effort)");
        }
    }

    /// The caller's vote on a held tool call: `POST /responses` (spec 029 §7.3).
    /// Same base URL and bearer as the turn; only the status is checked. `x-model-name` names
    /// the agent, as for a cancel, for a gateway that routes a request by it.
    pub async fn vote(
        &self,
        vote: &ApprovalVote,
        prompt_name: &str,
        bearer: &str,
        traceparent: Option<&str>,
    ) -> Result<(), TurnFailure> {
        // The turn's own `prompt.id`: budgateway's auth refuses a `/v1/responses` body with neither
        // `model` nor `prompt.id` (400), and with it the vote is authorised as the turn was.
        // budprompt takes the vote before any other branching, so it starts no run.
        let mut body = vote.body();
        body["prompt"] = json!({ "id": prompt_name });
        let mut req = self
            .http
            .post(format!("{}/responses", self.base_url))
            .bearer_auth(bearer)
            .header("x-model-name", prompt_name)
            .timeout(Duration::from_secs(10))
            .json(&body);
        if let Some(tp) = traceparent {
            req = req.header("traceparent", tp);
        }
        let resp = req.send().await.map_err(|e| {
            TurnFailure::Transport(format!("the Bud gateway could not be reached: {e}"))
        })?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let body = read_bounded(resp, MAX_ERROR_BODY_BYTES).await;
            return Err(classify_http(status, &body));
        }
        Ok(())
    }
}

async fn read_bounded(resp: reqwest::Response, max: usize) -> String {
    let mut out = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(Ok(chunk)) = stream.next().await {
        let room = max.saturating_sub(out.len());
        out.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if out.len() >= max {
            break;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Turn a byte stream into agent events. Ends after the terminal event.
pub fn events<S, E>(bytes: S) -> impl Stream<Item = Result<AgentEvent, TurnFailure>> + Send
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::fmt::Display,
{
    let state = (
        Box::pin(bytes),
        SseDecoder::default(),
        std::collections::VecDeque::<AgentEvent>::new(),
        false,
    );
    futures::stream::unfold(
        state,
        |(mut bytes, mut decoder, mut pending, mut done)| async move {
            loop {
                if let Some(ev) = pending.pop_front() {
                    let terminal = matches!(
                        ev,
                        AgentEvent::Completed { .. }
                            | AgentEvent::Failed { .. }
                            | AgentEvent::Incomplete { .. }
                    );
                    if terminal {
                        done = true;
                        pending.clear();
                    }
                    return Some((Ok(ev), (bytes, decoder, pending, done)));
                }
                if done {
                    return None;
                }
                match bytes.next().await {
                    Some(Ok(chunk)) => match decoder.push(&chunk) {
                        Ok(messages) => {
                            for (event, data) in messages {
                                if let Some(ev) = map_event(event.as_deref(), &data) {
                                    pending.push_back(ev);
                                }
                            }
                        }
                        Err(e) => {
                            done = true;
                            return Some((
                                Err(TurnFailure::Upstream(e)),
                                (bytes, decoder, pending, done),
                            ));
                        }
                    },
                    Some(Err(e)) => {
                        done = true;
                        return Some((
                            Err(TurnFailure::Upstream(format!(
                                "the agent stream broke: {e}"
                            ))),
                            (bytes, decoder, pending, done),
                        ));
                    }
                    None => {
                        done = true;
                        return Some((
                            Err(TurnFailure::Upstream(
                                "the agent stream ended before the run finished".into(),
                            )),
                            (bytes, decoder, pending, done),
                        ));
                    }
                }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 5xx that says the request never reached the agent is "unavailable", like a refused
    /// connection — and so retryable; any other 5xx (the agent ran and failed) is not.
    #[test]
    fn a_request_that_never_reached_the_agent_is_transport() {
        let body = r#"{"error":{"message":"Error sending request: error sending request for url (http://ditto-budprompt:3015/v1/responses)"}}"#;
        assert!(matches!(
            classify_http(500, body),
            TurnFailure::Transport(_)
        ));
        assert!(matches!(
            classify_http(502, "{}"),
            TurnFailure::Transport(_)
        ));
        assert!(matches!(
            classify_http(503, "{}"),
            TurnFailure::Transport(_)
        ));
        assert!(matches!(
            classify_http(504, "{}"),
            TurnFailure::Transport(_)
        ));
        let ran = r#"{"error":{"message":"the model returned an invalid tool call"}}"#;
        assert!(matches!(classify_http(500, ran), TurnFailure::Upstream(_)));
        assert!(TurnFailure::Transport(String::new()).retryable());
        assert!(!TurnFailure::Upstream(String::new()).retryable());
        assert!(!TurnFailure::RateLimited(String::new()).retryable());
    }

    fn sse(kind: &str, body: Value) -> String {
        let mut m = body.as_object().cloned().unwrap_or_default();
        m.insert("type".into(), json!(kind));
        format!("event: {kind}\ndata: {}\n\n", Value::Object(m))
    }

    #[test]
    fn the_request_names_the_agent_its_version_and_the_conversation() {
        let req = AgentTurnRequest {
            prompt_name: "support".into(),
            version: 3,
            variables: Some(Map::from_iter([("customer_id".to_string(), json!("c1"))])),
            input: "where is my order".into(),
            conversation_id: "conv_voice_s1".into(),
            truncate: Some(Truncate {
                response_id: "resp_1".into(),
                output_text: "Your order".into(),
            }),
            session_id: "s1".into(),
            turn_index: 4,
            extras: ClientExtras::default(),
            answer_marker: None,
        };
        let b = req.body();
        assert_eq!(
            b["prompt"],
            json!({"id": "support", "version": "3", "variables": {"customer_id": "c1"}})
        );
        assert_eq!(b["input"], "where is my order");
        assert_eq!(b["conversation"], "conv_voice_s1");
        assert_eq!(b["stream"], true);
        assert_eq!(b["bud_channel"], "voice");
        assert_eq!(
            b["bud_truncate"],
            json!({"response_id": "resp_1", "output_text": "Your order"})
        );
        assert_eq!(b["metadata"]["voice_turn_index"], "4");
        assert!(
            b.get("model").is_none(),
            "the agent is addressed by prompt, never by model"
        );
    }

    #[test]
    fn no_variables_and_no_truncate_are_omitted() {
        let req = AgentTurnRequest {
            prompt_name: "a".into(),
            version: 1,
            variables: Some(Map::new()),
            input: "hi".into(),
            conversation_id: "c".into(),
            truncate: None,
            session_id: "s".into(),
            turn_index: 0,
            extras: ClientExtras::default(),
            answer_marker: None,
        };
        let b = req.body();
        assert!(b["prompt"].get("variables").is_none());
        assert!(b.get("bud_truncate").is_none());
        // Nothing of a session's own is sent for a session that declared nothing.
        assert_eq!(b["input"], "hi");
        assert!(b.get("tools").is_none() && b.get("instructions").is_none());

        // A client tool's output goes ahead of the caller's words, with the session's tools.
        let output = json!({"type": "function_call_output", "call_id": "call_1", "output": "{}"});
        let tool = json!({"type": "function", "name": "delegate"});
        let with = AgentTurnRequest {
            extras: ClientExtras {
                tools: vec![tool.clone()],
                instructions: Some("Say hello.".into()),
                items: vec![output.clone()],
            },
            ..req.clone()
        };
        let b = with.body();
        assert_eq!(
            b["input"],
            json!([output, {"role": "user", "content": "hi"}])
        );
        assert_eq!(b["tools"], json!([tool]));
        assert_eq!(b["instructions"], "Say hello.");
        // A run the client asked for with only instructions has an empty list for its input.
        let injected = AgentTurnRequest {
            input: String::new(),
            extras: ClientExtras {
                instructions: Some("Say hello.".into()),
                ..ClientExtras::default()
            },
            ..req
        };
        assert_eq!(injected.body()["input"], json!([]));
    }

    #[test]
    fn the_answer_marker_is_sent_only_when_one_is_declared() {
        let req = AgentTurnRequest {
            prompt_name: "a".into(),
            version: 1,
            variables: None,
            input: "hi".into(),
            conversation_id: "c".into(),
            truncate: None,
            session_id: "s".into(),
            turn_index: 0,
            extras: ClientExtras::default(),
            answer_marker: Some("ANSWER:".into()),
        };
        assert_eq!(req.body()["bud_answer_marker"], "ANSWER:");
        // A plain voice agent's body (no marker) and an empty one carry no such field.
        for answer_marker in [None, Some(String::new())] {
            let body = AgentTurnRequest {
                answer_marker,
                ..req.clone()
            }
            .body();
            assert!(body.get("bud_answer_marker").is_none(), "{body}");
        }
    }

    #[test]
    fn a_function_call_carries_its_call_id_and_arguments() {
        let done = sse(
            "response.output_item.done",
            json!({"item": {"id": "fc_1", "type": "function_call", "name": "delegate",
                            "call_id": "call_1", "arguments": "{\"q\":1}", "status": "completed"}}),
        );
        let mut d = SseDecoder::default();
        let got: Vec<_> = d
            .push(done.as_bytes())
            .unwrap()
            .iter()
            .filter_map(|(e, data)| map_event(e.as_deref(), data))
            .collect();
        assert_eq!(
            got,
            vec![AgentEvent::FunctionCall {
                item_id: "fc_1".into(),
                name: "delegate".into(),
                ok: true,
                call_id: "call_1".into(),
                arguments: "{\"q\":1}".into(),
            }]
        );
    }

    #[test]
    fn sse_messages_split_anywhere_reassemble() {
        let raw = format!(
            "{}{}: keep-alive\n\n{}",
            sse(
                "response.created",
                json!({"response": {"id": "resp_9", "conversation": {"id": "conv_1"}}})
            ),
            sse(
                "response.output_text.delta",
                json!({"item_id": "msg_1", "delta": "Hel"})
            ),
            sse(
                "response.output_text.delta",
                json!({"item_id": "msg_1", "delta": "lo"})
            ),
        );
        let mut d = SseDecoder::default();
        let mut got = Vec::new();
        for chunk in raw.as_bytes().chunks(7) {
            got.extend(d.push(chunk).unwrap());
        }
        let events: Vec<_> = got
            .iter()
            .filter_map(|(e, data)| map_event(e.as_deref(), data))
            .collect();
        assert_eq!(
            events,
            vec![
                AgentEvent::Created {
                    response_id: "resp_9".into(),
                    conversation_id: Some("conv_1".into())
                },
                AgentEvent::TextDelta {
                    item_id: Some("msg_1".into()),
                    delta: "Hel".into()
                },
                AgentEvent::TextDelta {
                    item_id: Some("msg_1".into()),
                    delta: "lo".into()
                },
            ]
        );
    }

    #[test]
    fn crlf_and_multiline_data_are_handled() {
        let mut d = SseDecoder::default();
        let got = d.push(b"event: x\r\ndata: {\"type\":\r\ndata: \"error\",\"error\":{\"code\":\"c\",\"message\":\"m\"}}\r\n\r\n").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(
            map_event(got[0].0.as_deref(), &got[0].1),
            Some(AgentEvent::Failed {
                code: "c".into(),
                message: "m".into()
            })
        );
    }

    #[test]
    fn an_oversized_message_fails_the_stream() {
        let mut d = SseDecoder::default();
        let huge = vec![b'x'; MAX_SSE_MESSAGE_BYTES + 10];
        let mut raw = b"data: ".to_vec();
        raw.extend(huge);
        assert!(d.push(&raw).is_err());
    }

    #[test]
    fn tools_start_and_finish_with_their_names() {
        let added = sse(
            "response.output_item.added",
            json!({"item": {"id": "mcp_1", "type": "mcp_call", "name": "lookup_order", "server_label": "orders"}}),
        );
        let done = sse(
            "response.output_item.done",
            json!({"item": {"id": "mcp_1", "type": "mcp_call", "name": "lookup_order", "status": "failed", "error": "boom"}}),
        );
        let msg = sse(
            "response.output_item.added",
            json!({"item": {"id": "msg_1", "type": "message"}}),
        );
        let mut d = SseDecoder::default();
        let got: Vec<_> = d
            .push(format!("{added}{done}{msg}").as_bytes())
            .unwrap()
            .iter()
            .filter_map(|(e, data)| map_event(e.as_deref(), data))
            .collect();
        assert_eq!(
            got,
            vec![
                AgentEvent::ToolStarted {
                    item_id: "mcp_1".into(),
                    name: "lookup_order".into(),
                    kind: "mcp_call".into()
                },
                AgentEvent::ToolFinished {
                    item_id: "mcp_1".into(),
                    name: "lookup_order".into(),
                    ok: false,
                    error: Some("boom".into()),
                },
            ]
        );
    }

    #[test]
    fn a_failed_tool_keeps_what_it_said_went_wrong() {
        let finished = |item: Value| match map_event(
            None,
            &json!({"type": "response.output_item.done", "item": item}).to_string(),
        ) {
            Some(AgentEvent::ToolFinished { ok, error, .. }) => (ok, error),
            other => panic!("not a finished tool: {other:?}"),
        };
        // An error object's message.
        assert_eq!(
            finished(json!({"id": "m1", "type": "mcp_call", "name": "t",
                            "error": {"type": "tool_error", "message": "upstream 503"}})),
            (false, Some("upstream 503".into()))
        );
        // An error object without a message is kept as its JSON.
        assert_eq!(
            finished(json!({"id": "m1", "type": "mcp_call", "name": "t", "error": {"code": "x"}})),
            (false, Some(r#"{"code":"x"}"#.into()))
        );
        // Failed without saying why: no text is made up.
        assert_eq!(
            finished(json!({"id": "m1", "type": "web_search_call", "status": "failed"})),
            (false, None)
        );
        // A tool that succeeded has no error, even with an empty `error` field.
        assert_eq!(
            finished(
                json!({"id": "m1", "type": "mcp_call", "name": "t", "status": "completed", "error": null})
            ),
            (true, None)
        );
    }

    #[test]
    fn terminal_frames_carry_usage_and_output() {
        let completed = map_event(
            None,
            &json!({"type": "response.completed", "response": {"usage": {"input_tokens": 3}, "output": [{"type": "message"}]}}).to_string(),
        );
        assert_eq!(
            completed,
            Some(AgentEvent::Completed {
                usage: Some(json!({"input_tokens": 3})),
                output: vec![json!({"type": "message"})]
            })
        );
        let incomplete = map_event(
            None,
            &json!({"type": "response.incomplete", "response": {"status": "cancelled", "output": []}}).to_string(),
        );
        assert!(
            matches!(incomplete, Some(AgentEvent::Incomplete { status, .. }) if status == "cancelled")
        );
        let failed = map_event(
            None,
            &json!({"type": "response.failed", "response": {"error": {"code": "server_error", "message": "upstream died"}}}).to_string(),
        );
        assert_eq!(
            failed,
            Some(AgentEvent::Failed {
                code: "server_error".into(),
                message: "upstream died".into()
            })
        );
    }

    /// Spec 029 §7.3: the hold is an `mcp_approval_request` item; its `done` frame is not a
    /// second hold, and it is never a tool that started.
    #[test]
    fn an_approval_hold_is_parsed_with_its_deadline() {
        let item = json!({"type": "mcp_approval_request", "id": "esc_1", "name": "refund_order",
                          "arguments": "{\"order_id\":\"4417\"}", "server_label": "orders",
                          "bud_expires_at": 1_762_000_030});
        let added = sse(
            "response.output_item.added",
            json!({"output_index": 1, "sequence_number": 7, "item": item}),
        );
        let done = sse("response.output_item.done", json!({"item": item}));
        let mut d = SseDecoder::default();
        let got: Vec<_> = d
            .push(format!("{added}: keep-alive\n\n{done}").as_bytes())
            .unwrap()
            .iter()
            .filter_map(|(e, data)| map_event(e.as_deref(), data))
            .collect();
        assert_eq!(
            got,
            vec![AgentEvent::ApprovalRequest {
                id: "esc_1".into(),
                name: "refund_order".into(),
                arguments: "{\"order_id\":\"4417\"}".into(),
                server_label: Some("orders".into()),
                expires_at: Some(1_762_000_030.0),
            }]
        );
        // Without a deadline or arguments it is still a hold.
        let bare = json!({"type": "response.output_item.added",
                          "item": {"type": "mcp_approval_request", "id": "esc_2", "name": "t"}});
        assert_eq!(
            map_event(None, &bare.to_string()),
            Some(AgentEvent::ApprovalRequest {
                id: "esc_2".into(),
                name: "t".into(),
                arguments: String::new(),
                server_label: None,
                expires_at: None,
            })
        );
    }

    /// Spec 029 §7.3: a run held for an approval only a designated person can give ends with
    /// `response.failed` / `approval_required`: the approval failure, not an upstream error.
    #[test]
    fn a_run_that_failed_for_approval_is_the_approval_failure() {
        let failed = map_event(
            None,
            &json!({"type": "response.failed", "response": {"error": {"code": "approval_required", "message": "a manager must approve refunds"}}}).to_string(),
        );
        let Some(AgentEvent::Failed { code, message }) = failed else {
            panic!("{failed:?}");
        };
        let failure = TurnFailure::of_run(&code, &message);
        assert_eq!(
            failure,
            TurnFailure::ApprovalRequired("a manager must approve refunds".into())
        );
        assert_eq!(failure.code(), "approval_required");
        assert_eq!(
            TurnFailure::of_run("server_error", "upstream died"),
            TurnFailure::Upstream("server_error: upstream died".into())
        );
    }

    /// Spec 029 §7.3: the vote's body, exactly.
    #[test]
    fn the_vote_is_an_approval_response_continuing_the_turn() {
        let vote = ApprovalVote {
            previous_response_id: "resp_1".into(),
            approval_request_id: "esc_1".into(),
            approve: true,
            reason: None,
        };
        assert_eq!(
            vote.body(),
            json!({"previous_response_id": "resp_1", "stream": false,
                   "input": [{"type": "mcp_approval_response", "approval_request_id": "esc_1",
                              "approve": true}]})
        );
        let declined = ApprovalVote {
            approve: false,
            reason: Some("timeout".into()),
            ..vote
        };
        let body = declined.body();
        assert_eq!(body["input"][0]["approve"], false);
        assert_eq!(body["input"][0]["reason"], "timeout");
        assert!(body.get("conversation").is_none() && body.get("prompt").is_none());
    }

    #[test]
    fn noise_is_ignored() {
        for kind in [
            "response.reasoning_text.delta",
            "response.mcp_list_tools.completed",
            "response.content_part.added",
        ] {
            assert_eq!(map_event(None, &json!({"type": kind}).to_string()), None);
        }
        assert_eq!(map_event(None, "[DONE]"), None);
        assert_eq!(map_event(None, "not json"), None);
    }

    #[test]
    fn http_failures_are_classified_per_the_frd() {
        let body =
            |code: &str, msg: &str| json!({"error": {"code": code, "message": msg}}).to_string();
        assert!(matches!(
            classify_http(429, "{}"),
            TurnFailure::RateLimited(_)
        ));
        assert!(matches!(
            classify_http(401, &body("invalid_api_key", "x")),
            TurnFailure::Unauthorized(_)
        ));
        assert!(matches!(
            classify_http(403, &body("approval_required_foreground", "needs approval")),
            TurnFailure::ApprovalRequired(m) if m == "needs approval"
        ));
        assert!(matches!(
            classify_http(403, &body("policy_denied", "no")),
            TurnFailure::Forbidden(_)
        ));
        assert!(matches!(
            classify_http(404, &body("not_found", "Prompt not found: x")),
            TurnFailure::AgentNotFound(_)
        ));
        assert!(matches!(
            classify_http(400, &body("prompt_variable_missing", "missing customer_id")),
            TurnFailure::InvalidVariables(_)
        ));
        assert!(matches!(
            classify_http(
                400,
                &json!({"error": {"param": "prompt.variables.age", "message": "bad"}}).to_string()
            ),
            TurnFailure::InvalidVariables(_)
        ));
        assert!(matches!(
            classify_http(400, &body("x", "y")),
            TurnFailure::BadRequest(_)
        ));
        assert!(
            matches!(classify_http(502, "<html>"), TurnFailure::Transport(m) if m.contains("502"))
        );
        assert_eq!(classify_http(401, "{}").code(), "auth_expired");
        assert!(classify_http(503, "").speaks_degradation());
        assert!(
            !classify_http(403, &body("approval_required_foreground", "x")).speaks_degradation()
        );
    }

    #[tokio::test]
    async fn the_event_stream_ends_at_the_terminal_frame() {
        let frames = format!(
            "{}{}{}",
            sse("response.output_text.delta", json!({"delta": "a"})),
            sse("response.completed", json!({"response": {"output": []}})),
            sse("response.output_text.delta", json!({"delta": "after"})),
        );
        let bytes = futures::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(frames))]);
        let got: Vec<_> = events(bytes).collect().await;
        assert_eq!(got.len(), 2);
        assert!(matches!(got[1], Ok(AgentEvent::Completed { .. })));
    }

    #[tokio::test]
    async fn a_stream_that_ends_early_is_an_upstream_failure() {
        let frames = sse("response.output_text.delta", json!({"delta": "a"}));
        let bytes = futures::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(frames))]);
        let got: Vec<_> = events(bytes).collect().await;
        assert_eq!(got.len(), 2);
        assert!(matches!(&got[1], Err(TurnFailure::Upstream(m)) if m.contains("ended before")));
    }
}
