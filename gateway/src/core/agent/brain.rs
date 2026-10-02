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
    /// That tool finished.
    ToolFinished {
        item_id: String,
        name: String,
        ok: bool,
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
                Some(AgentEvent::ToolFinished {
                    item_id,
                    name,
                    ok: !failed,
                })
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
    /// every other failure is ignored — closing the stream already stopped the model.
    pub async fn cancel(&self, response_id: &str, prompt_name: &str, bearer: &str) {
        let url = format!("{}/responses/{}/cancel", self.base_url, response_id);
        let res = self
            .http
            .post(url)
            .bearer_auth(bearer)
            .header("x-model-name", prompt_name)
            .timeout(Duration::from_secs(5))
            .send()
            .await;
        if let Err(e) = res {
            tracing::debug!(response_id, error = %e, "agent run cancel failed (best-effort)");
        }
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
        };
        let b = req.body();
        assert!(b["prompt"].get("variables").is_none());
        assert!(b.get("bud_truncate").is_none());
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
                    ok: false
                },
            ]
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
